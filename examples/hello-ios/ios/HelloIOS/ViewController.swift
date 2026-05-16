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
