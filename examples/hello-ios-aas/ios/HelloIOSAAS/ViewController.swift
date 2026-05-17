import UIKit

/// AAS dev-server URL. Defaults to host loopback (works for the
/// iOS simulator hitting a dev-server running on the Mac). Override
/// via `Info.plist` key `IDEALYST_AAS_URL` for a device running on
/// the same LAN as the dev machine.
private func resolveDevServerURL() -> String {
    if let s = Bundle.main.object(forInfoDictionaryKey: "IDEALYST_AAS_URL") as? String,
       !s.isEmpty
    {
        return s
    }
    return "ws://127.0.0.1:9001"
}

class ViewController: UIViewController {
    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black

        let rootPtr = Unmanaged.passUnretained(view).toOpaque()
        let urlString = resolveDevServerURL()
        urlString.withCString { cstr in
            ios_main(rootPtr, cstr)
        }
    }

    deinit {
        ios_teardown()
    }
}
