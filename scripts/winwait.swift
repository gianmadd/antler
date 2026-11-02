// winwait PID [TIMEOUT_S]: waits until a process in PID's tree (PID or a descendant) owns an on-screen
// normal window; prints the seconds elapsed. Owner PIDs need no Screen Recording permission.
import CoreGraphics
import Foundation

let args = CommandLine.arguments
guard args.count >= 2, let root = Int32(args[1]) else { print("usage: winwait PID [TIMEOUT_S]"); exit(2) }
let timeout = args.count > 2 ? Double(args[2]) ?? 60 : 60

func tree(_ pid: Int32) -> Set<Int32> {
    let p = Process(); p.executableURL = URL(fileURLWithPath: "/usr/bin/pgrep"); p.arguments = ["-P", String(pid)]
    let out = Pipe(); p.standardOutput = out; try? p.run(); p.waitUntilExit()
    let kids = String(data: out.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8)!
        .split(separator: "\n").compactMap { Int32($0) }
    return kids.reduce(into: [pid]) { $0.formUnion(tree($1)) }
}

let start = Date()
while Date().timeIntervalSince(start) < timeout {
    let pids = tree(root)
    let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
    if windows.contains(where: { w in
        guard let pid = w[kCGWindowOwnerPID as String] as? Int32, pids.contains(pid),
              (w[kCGWindowLayer as String] as? Int) == 0,
              let b = w[kCGWindowBounds as String] as? [String: Double], (b["Width"] ?? 0) > 400 else { return false }
        return true
    }) {
        print(String(format: "%.2f", Date().timeIntervalSince(start))); exit(0)
    }
    usleep(20_000)
}
print("timeout"); exit(1)
