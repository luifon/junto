import CoreGraphics
import Foundation
let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as! [[String: Any]]
var counts: [String: Int] = [:]
for w in list {
    let owner = w[kCGWindowOwnerName as String] as? String ?? "?"
    let layer = w[kCGWindowLayer as String] as? Int ?? 0
    counts["\(owner) layer=\(layer)", default: 0] += 1
}
for (k, v) in counts.sorted(by: { $0.key < $1.key }) { print(v, k) }
