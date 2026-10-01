import Foundation

// Queue ownership stays on the network queue. Never discard the head mid-send
// merely because a newer frame arrived; wait for an independent picture after loss.
struct MediaSendQueue {
    struct Pending { let unit: EncodedUnit; var offset = 0; let created: UInt64 }
    private(set) var items: [Pending] = []
    private(set) var waitingForIDR = false
    let video: Bool
    var bytes: Int { items.reduce(0) { $0 + $1.unit.bytes.count } }
    var first: Pending? { items.first }
    mutating func enqueue(_ unit: EncodedUnit, now: UInt64) -> Bool {
        guard !unit.bytes.isEmpty, unit.bytes.count <= (video ? 8 * 1024 * 1024 : 65_536) else { return true }
        if video && waitingForIDR && !unit.independent { return false }
        var lost = false
        if items.count >= (video ? 3 : 10) || bytes + unit.bytes.count > (video ? 24 * 1024 * 1024 : 655_360) {
            // A partly submitted access unit must finish; queued dependent pictures
            // may be replaced only by an IDR. The head itself remains bounded by age.
            let head = items.first.flatMap { $0.offset > 0 ? $0 : nil }
            items = head.map { [$0] } ?? []
            lost = video; waitingForIDR = video
            if video && !unit.independent { return true }
        }
        if unit.independent { waitingForIDR = false }
        items.append(Pending(unit: unit, created: now)); return lost
    }
    mutating func expire(now: UInt64) -> Bool {
        guard let first, now >= first.created, now - first.created > 100_000_000 else { return false }
        items.removeAll(keepingCapacity: true); waitingForIDR = video; return video
    }
    mutating func consume(_ count: Int) {
        guard !items.isEmpty else { return }; items[0].offset += count
        if items[0].offset >= items[0].unit.bytes.count { items.removeFirst() }
    }
    mutating func lose() { items.removeAll(keepingCapacity: true); waitingForIDR = video }
}

// Average rate includes framing/audio allowance. A separate peak bucket gives an
// IDR bounded headroom rather than forcing every picture to average-frame size.
struct MediaPacer {
    private(set) var average: Double = 2_000_000
    private(set) var peak: Double = 6_000_000
    private var tokens: Double = 0
    private var peakTokens: Double = 0
    private var last: UInt64?
    mutating func configure(bitrate: Int, wireBudgetMbps: Int = 200) {
        average = (Double(bitrate) * 1.10 + 400_000) / 8
        peak = min(Double(wireBudgetMbps) * 1_000_000 / 8, average * 4)
        tokens = min(tokens, min(2_000_000, average * 0.25)); peakTokens = min(peakTokens, peak * 0.005)
    }
    mutating func advance(now: UInt64) {
        if let last, now >= last {
            let elapsed = Double(now - last) / 1e9
            tokens = min(min(2_000_000, average * 0.25), tokens + elapsed * average)
            peakTokens = min(peak * 0.005, peakTokens + elapsed * peak)
        } else { tokens = min(2_000_000, average * 0.25); peakTokens = peak * 0.005 }
        last = now
    }
    mutating func take(bytes: Int, audio: Bool = false) -> Bool {
        let size = Double(bytes)
        let reserve: Double = audio ? 0 : 4_096
        guard peakTokens >= size, tokens >= size + reserve else { return false }
        tokens -= size; peakTokens -= size; return true
    }
}
