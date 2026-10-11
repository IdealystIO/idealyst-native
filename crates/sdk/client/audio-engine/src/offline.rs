//! [`OfflineRenderer`] — the Rust mixer without a device.

use std::rc::Rc;

use crate::backend::{EngineBackend, NoDevice};
use crate::mixer::{self, Mixer, MixerEngine};

/// Pulls audio out of an [`AudioEngine::offline`](crate::AudioEngine::offline)
/// engine. Each `render*` call drains the commands the engine has sent so
/// far (at render-quantum boundaries, exactly as a device would) and renders
/// the next frames. `Send`: render on whichever thread you like.
pub struct OfflineRenderer {
    mixer: Mixer,
    sample_rate: u32,
    channels: u16,
}

pub(crate) fn new(sample_rate: u32, channels: u16) -> (Rc<dyn EngineBackend>, OfflineRenderer) {
    let sample_rate = sample_rate.max(1);
    let channels = channels.max(1);
    let queue = mixer::new_queue();
    let renderer = OfflineRenderer {
        mixer: Mixer::new(queue.clone(), sample_rate),
        sample_rate,
        channels,
    };
    let engine = MixerEngine::new(queue, sample_rate, Box::new(NoDevice));
    (Rc::new(engine), renderer)
}

impl OfflineRenderer {
    /// Fill `out` (interleaved, [`channels`](Self::channels) wide).
    pub fn render(&mut self, out: &mut [f32]) {
        self.mixer.render(out, self.channels, self.sample_rate);
    }

    /// Render the next `frames` frames.
    pub fn render_frames(&mut self, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames * self.channels as usize];
        self.render(&mut out);
        out
    }

    /// Render the next `secs` seconds (rounded to whole frames).
    pub fn render_secs(&mut self, secs: f64) -> Vec<f32> {
        let frames = (secs.max(0.0) * self.sample_rate as f64).round() as usize;
        self.render_frames(frames)
    }

    /// Output sample rate.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Output channel count.
    pub fn channels(&self) -> u16 {
        self.channels
    }
}
