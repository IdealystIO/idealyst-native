import UIKit

class ViewController: UIViewController {
    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .white

        // Pass our root view to the Rust framework. It will add its
        // view tree as subviews of this view.
        let rawPtr = Unmanaged.passUnretained(view).toOpaque()
        ios_main(rawPtr)

        // Debug: dump view hierarchy after a short delay so layout runs
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.0) { [weak self] in
            guard let self = self else { return }
            self.dumpView(self.view, indent: 0)
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
