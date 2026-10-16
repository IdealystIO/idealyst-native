//! Windows playback — Media Foundation's Media Engine.
//!
//! The same engine the video SDK uses, audio-only (no Direct3D device): it
//! decodes what Windows has codecs for (wav, mp3, aac/m4a, flac, wma …) and
//! streams `http(s)` URLs progressively, like the mobile players and the
//! browser.
//!
//! - **`prepare`** opens the source on an engine and waits for its
//!   `LOADEDMETADATA` (ready) or `ERROR` (an undecodable or unreachable
//!   source → [`AudioError::Decode`]) event, so a bad sound fails at
//!   [`load`](crate::load) as on the other platforms. Those events arrive on
//!   Media Foundation's threads and cross to the awaiting task over a
//!   channel. That engine is then dropped; the prepared sound keeps the
//!   source.
//! - **`play`** starts a fresh engine per call, so plays overlap. In-memory
//!   bytes reach each engine as an `IMFByteStream` over a memory stream;
//!   paths and URLs are set directly.
//! - The handle drives the engine (`Pause` / `Play` / `SetVolume` /
//!   `SetLoop`); dropping it shuts the engine down, which stops the sound.

use std::sync::{Arc, Mutex};

use windows::core::{implement, Interface, Result as WinResult, BSTR, HRESULT};
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFByteStream, IMFMediaEngine, IMFMediaEngineClassFactory, IMFMediaEngineEx,
    IMFMediaEngineNotify, IMFMediaEngineNotify_Impl, MFCreateAttributes, MFCreateMFByteStreamOnStream,
    MFStartup, CLSID_MFMediaEngineClassFactory, MFSTARTUP_FULL, MF_MEDIA_ENGINE_CALLBACK,
    MF_MEDIA_ENGINE_EVENT_ERROR, MF_MEDIA_ENGINE_EVENT_LOADEDMETADATA, MF_VERSION,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Shell::SHCreateMemStream;

use crate::{AudioError, AudioSource};

/// The engine's verdict on a source: ready, or the error's HRESULT.
type Verdict = std::result::Result<(), HRESULT>;

/// Forwards the engine's ready / error event to whoever is waiting.
#[implement(IMFMediaEngineNotify)]
struct Notify {
    tx: Mutex<Option<async_channel::Sender<Verdict>>>,
}

impl IMFMediaEngineNotify_Impl for Notify_Impl {
    fn EventNotify(&self, event: u32, _param1: usize, param2: u32) -> WinResult<()> {
        let verdict = if event == MF_MEDIA_ENGINE_EVENT_LOADEDMETADATA.0 as u32 {
            Ok(())
        } else if event == MF_MEDIA_ENGINE_EVENT_ERROR.0 as u32 {
            Err(HRESULT(param2 as i32))
        } else {
            return Ok(());
        };
        if let Some(tx) = self.tx.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = tx.try_send(verdict);
        }
        Ok(())
    }
}

/// Where a prepared sound's audio comes from.
#[derive(Clone)]
enum Source {
    Bytes(Arc<Vec<u8>>),
    /// A path or a URL — both are what `SetSource` takes.
    Location(String),
}

pub(crate) struct PreparedSound {
    source: Source,
}

impl PreparedSound {
    pub(crate) fn play(&self) -> PlaybackHandle {
        let engine = unsafe { open(&self.source, None) }.ok();
        if let Some(e) = &engine {
            let _ = unsafe { e.Play() };
        }
        PlaybackHandle { engine }
    }
}

/// A running playback. `None` when the engine could not be created (the
/// handle is then inert, never a panic).
pub(crate) struct PlaybackHandle {
    engine: Option<IMFMediaEngine>,
}

impl PlaybackHandle {
    pub(crate) fn pause(&self) {
        if let Some(e) = &self.engine {
            let _ = unsafe { e.Pause() };
        }
    }
    pub(crate) fn resume(&self) {
        if let Some(e) = &self.engine {
            let _ = unsafe { e.Play() };
        }
    }
    pub(crate) fn set_volume(&self, volume: f32) {
        if let Some(e) = &self.engine {
            let _ = unsafe { e.SetVolume(volume.clamp(0.0, 1.0) as f64) };
        }
    }
    pub(crate) fn set_looping(&self, looping: bool) {
        if let Some(e) = &self.engine {
            let _ = unsafe { e.SetLoop(looping) };
        }
    }
    pub(crate) fn is_playing(&self) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|e| unsafe { !e.IsPaused().as_bool() && !e.IsEnded().as_bool() })
    }
}

impl Drop for PlaybackHandle {
    fn drop(&mut self) {
        if let Some(e) = self.engine.take() {
            let _ = unsafe { e.Shutdown() };
        }
    }
}

pub(crate) async fn prepare(source: AudioSource) -> Result<PreparedSound, AudioError> {
    let source = match source {
        AudioSource::Bytes(b) => Source::Bytes(Arc::new(b)),
        AudioSource::Path(p) => {
            if !p.is_file() {
                return Err(AudioError::Backend(format!("no such file: {}", p.display())));
            }
            Source::Location(p.to_string_lossy().into_owned())
        }
        AudioSource::Url(u) => Source::Location(u),
    };
    let (tx, rx) = async_channel::bounded(1);
    let engine = unsafe { open(&source, Some(tx)) }.map_err(|e| AudioError::Backend(format!("Media Foundation: {e}")))?;
    let verdict = rx.recv().await;
    let _ = unsafe { engine.Shutdown() };
    match verdict {
        Ok(Ok(())) => Ok(PreparedSound { source }),
        Ok(Err(hr)) => Err(AudioError::Decode(format!("Media Foundation could not open it ({:#010x})", hr.0))),
        Err(_) => Err(AudioError::Backend("the engine went away before it opened the source".into())),
    }
}

/// An engine on `source`, reporting ready / error to `ready` when given.
unsafe fn open(source: &Source, ready: Option<async_channel::Sender<Verdict>>) -> WinResult<IMFMediaEngine> {
    // COM on this thread (the UI thread's apartment, or a test's).
    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
    let mut attrs: Option<IMFAttributes> = None;
    MFCreateAttributes(&mut attrs, 1)?;
    let attrs = attrs.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))?;
    let notify: IMFMediaEngineNotify = Notify { tx: Mutex::new(ready) }.into();
    attrs.SetUnknown(&MF_MEDIA_ENGINE_CALLBACK, &notify)?;
    let factory: IMFMediaEngineClassFactory =
        CoCreateInstance(&CLSID_MFMediaEngineClassFactory, None, CLSCTX_INPROC_SERVER)?;
    // `MF_MEDIA_ENGINE_AUDIOONLY`: no video device needed.
    let engine = factory.CreateInstance(0x1, &attrs)?;
    match source {
        Source::Location(s) => engine.SetSource(&BSTR::from(s.as_str()))?,
        Source::Bytes(bytes) => {
            let stream = SHCreateMemStream(Some(bytes)).ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_OUTOFMEMORY))?;
            let byte_stream: IMFByteStream = MFCreateMFByteStreamOnStream(&stream)?;
            // The URL is only a hint for format detection; the stream's
            // content is sniffed either way.
            engine.cast::<IMFMediaEngineEx>()?.SetSourceFromByteStream(&byte_stream, &BSTR::from("memory"))?;
        }
    }
    Ok(engine)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 0.2 s, 8 kHz, 16-bit mono silent WAV.
    fn wav() -> Vec<u8> {
        let samples = 1600u32;
        let data_len = samples * 2;
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + data_len).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes()); // PCM
        v.extend_from_slice(&1u16.to_le_bytes()); // mono
        v.extend_from_slice(&8000u32.to_le_bytes());
        v.extend_from_slice(&16000u32.to_le_bytes());
        v.extend_from_slice(&2u16.to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&data_len.to_le_bytes());
        v.resize(v.len() + data_len as usize, 0);
        v
    }

    /// The bug: Windows fell to the NotSupported stub, so `load` always
    /// failed. A WAV in memory loads and plays; garbage fails to load as `Decode`, a
    /// missing file as `Backend`.
    #[test]
    fn regression_windows_loads_and_plays_audio() {
        let sound = pollster::block_on(crate::load(AudioSource::bytes(wav()))).expect("a WAV loads");
        let playback = sound.play();
        playback.set_volume(0.0);
        playback.set_looping(true);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !playback.is_playing() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(playback.is_playing(), "a looping sound plays");
        playback.pause();
        assert!(!playback.is_playing());
        playback.stop();

        let bad = pollster::block_on(crate::load(AudioSource::bytes(b"not audio at all".to_vec())));
        assert!(matches!(bad, Err(AudioError::Decode(_))), "{:?}", bad.err());
        let missing = pollster::block_on(crate::load(AudioSource::path("C:/no/such/sound.wav")));
        assert!(matches!(missing, Err(AudioError::Backend(_))), "{:?}", missing.err());
    }
}
