import UIKit

/// Info.plist key carrying the expected `app_id`. The Rust side
/// browses Bonjour for `_idealyst-dev._tcp.` and connects to the
/// dev-server whose TXT record's `app_id` matches. Two dev-servers
/// on the same network with different `app_id`s don't cross-wire.
private let kAppIdInfoKey = "IdealystAppId"

class ViewController: UIViewController {
    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black

        guard let appId = Bundle.main
                .object(forInfoDictionaryKey: kAppIdInfoKey) as? String,
              !appId.isEmpty
        else {
            fatalError(
                "Missing or empty `\(kAppIdInfoKey)` in Info.plist — the app "
                + "doesn't know which dev-server to look for. Set it to the "
                + "same string the dev-server's bin passes as APP_ID."
            )
        }

        // Rust handles discovery + WebSocket. We just hand it the
        // root view and the app id; everything else (mDNS browsing,
        // reconnect-on-port-change, frame pump) lives in
        // `hello-ios-aas`'s `ios_main`.
        let rootPtr = Unmanaged.passUnretained(view).toOpaque()
        appId.withCString { cstr in
            ios_main(rootPtr, cstr)
        }
    }

    deinit {
        ios_teardown()
    }
}
