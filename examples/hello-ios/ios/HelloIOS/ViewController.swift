import UIKit

class ViewController: UIViewController {
    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .white

        let rawPtr = Unmanaged.passUnretained(view).toOpaque()
        ios_main(rawPtr)

        // Debug: dump view hierarchy after delay
        DispatchQueue.main.asyncAfter(deadline: .now() + 2.0) { [weak self] in
            guard let self = self else { return }
            print("=== VIEW HIERARCHY ===")
            self.dumpView(self.view.window ?? self.view, indent: 0)
            print("=== END ===")
        }
    }

    private func dumpView(_ view: UIView, indent: Int) {
        let prefix = String(repeating: "  ", count: indent)
        let cls = String(describing: type(of: view))
        let f = view.frame
        let subs = view.subviews.count
        print("\(prefix)\(cls) frame=(\(Int(f.origin.x)),\(Int(f.origin.y)),\(Int(f.size.width)),\(Int(f.size.height))) subs=\(subs) hidden=\(view.isHidden) alpha=\(view.alpha)")
        for sub in view.subviews {
            dumpView(sub, indent: indent + 1)
        }
    }

    deinit {
        ios_teardown()
    }
}
