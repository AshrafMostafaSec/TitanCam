import Foundation

// One complete access unit in NWConnection at a time, plus a bounded fresh queue.
// Temporary backpressure repairs the GOP; it does not tear down the control link.
final class USBMediaSender {
    private let sendRecord: (Data, @escaping () -> Void) -> Bool
    private let clock: () -> UInt64
    private var pending: MediaSendQueue
    private var sending = false
    private var stopped = false
    private var lossAt: UInt64 = 0
    var dropped: (() -> Void)?
    convenience init(connection: FramedConnection, video: Bool) {
        self.init(video: video, send: { data, done in connection.send(data, media: true, completion: done) }, now: { CaptureEngine.hostTime })
    }
    init(video: Bool, send: @escaping (Data, @escaping () -> Void) -> Bool, now: @escaping () -> UInt64) {
        sendRecord = send; clock = now; pending = MediaSendQueue(video: video)
    }
    func enqueue(_ unit: EncodedUnit) {
        guard !stopped else { return }; let now = clock()
        let expired = pending.expire(now: now)
        let lost = pending.enqueue(unit, now: now)
        if expired || lost { reportLoss(now) }
        pump()
    }
    private func reportLoss(_ now: UInt64) {
        guard lossAt == 0 || now - lossAt >= 250_000_000 else { return }
        lossAt = now; dropped?()
    }
    private func pump() {
        guard !stopped, !sending else { return }
        if pending.expire(now: clock()) { reportLoss(clock()) }
        guard let first = pending.first else { return }
        sending = true; pending.consume(first.unit.bytes.count)
        if !sendRecord(first.unit.packet(), { [weak self] in
            guard let self, !self.stopped else { return }; self.sending = false; self.pump()
        }) {
            sending = false; pending.lose(); reportLoss(clock())
        }
    }
    func stop() { stopped = true; pending.lose(); dropped = nil }
}
