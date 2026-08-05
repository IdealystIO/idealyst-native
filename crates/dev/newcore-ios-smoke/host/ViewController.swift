// Swift shell for newcore-ios-smoke — mirrors the CLI-generated
// wrapper template (crates/tools/run/ios/templates/
// ViewController.swift) minus the splash screen. `ios_main` here is
// the smoke staticlib's own entry (backend_ios::newcore::run_in_view),
// same C ABI as the generated wrapper's.

import UIKit

class ViewController: UIViewController {
    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .white
        let rawPtr = Unmanaged.passUnretained(view).toOpaque()
        ios_main(rawPtr)
    }

    deinit {
        ios_teardown()
    }
}
