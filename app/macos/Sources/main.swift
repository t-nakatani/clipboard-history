import AppKit

if CommandLine.arguments.contains("--self-test") {
    exit(runSelfTest())
}

let application = NSApplication.shared
let delegate = AppDelegate()
application.delegate = delegate
application.setActivationPolicy(.accessory)
application.run()
