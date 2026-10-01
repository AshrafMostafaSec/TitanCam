import XCTest
import Network
@testable import TitanCam
final class MediaSendQueueTests: XCTestCase {
    private func unit(_ n: UInt64, independent: Bool = false, size: Int = 2000, kind: UInt8 = 1) -> EncodedUnit {
        EncodedUnit(kind: kind, independent: independent, session: Data(repeating: 1, count: 16), config: 1, sequence: n, pts: n * 16_666_667, duration: 16_666_667, bytes: Data(repeating: 7, count: size))
    }
    func testOverflowFinishesPartialHeadAndWaitsForIndependentPicture() {
        var q = MediaSendQueue(video: true)
        XCTAssertFalse(q.enqueue(unit(0, independent: true), now: 1)); q.consume(1036)
        XCTAssertFalse(q.enqueue(unit(1), now: 2)); XCTAssertFalse(q.enqueue(unit(2), now: 3))
        XCTAssertTrue(q.enqueue(unit(3), now: 4))
        XCTAssertEqual(q.items.count, 1); XCTAssertEqual(q.first?.offset, 1036)
        XCTAssertTrue(q.waitingForIDR)
        XCTAssertFalse(q.enqueue(unit(4), now: 5)); XCTAssertEqual(q.items.count, 1)
        XCTAssertFalse(q.enqueue(unit(5, independent: true), now: 6)); XCTAssertFalse(q.waitingForIDR)
        q.consume(964); XCTAssertEqual(q.first?.unit.sequence, 5)
    }
    func testDeadlineAbandonsDamagedGOPAndAudioCannotGrowWithoutBound() {
        var q = MediaSendQueue(video: true)
        _ = q.enqueue(unit(0, independent: true), now: 1); q.consume(1036)
        XCTAssertTrue(q.expire(now: 100_000_002)); XCTAssertTrue(q.items.isEmpty)
        _ = q.enqueue(unit(1), now: 100_000_003); XCTAssertTrue(q.items.isEmpty)
        _ = q.enqueue(unit(2, independent: true), now: 100_000_004); XCTAssertEqual(q.items.count, 1)
        var audio = MediaSendQueue(video: false)
        for n in 0..<100 { _ = audio.enqueue(unit(UInt64(n), kind: 3), now: UInt64(n)) }
        XCTAssertLessThanOrEqual(audio.items.count, 10); XCTAssertLessThanOrEqual(audio.bytes, 655_360)
    }
    func testAllProfilesSustainConfiguredRateWithoutEightPacketTimerCeiling() {
        for (bitrate, period) in [(4_000_000, 5), (14_000_000, 2), (65_000_000, 1)] {
            var p = MediaPacer(); p.configure(bitrate: bitrate); var bytes = 0
            for tick in 0..<(1000 / period) {
                p.advance(now: UInt64(tick * period) * 1_000_000)
                for _ in 0..<128 { if p.take(bytes: 1100) { bytes += 1100 } else { break } }
            }
            XCTAssertGreaterThan(Double(bytes), p.average * 0.95)
            XCTAssertLessThanOrEqual(Double(bytes), p.average * 1.26)
        }
    }
    func testLargeIDRCompletesInsideReassemblyDeadlineAndPeakIsBounded() {
        for (bitrate, size) in [(4_000_000, 100_000), (14_000_000, 300_000), (65_000_000, 1_000_000)] {
            var p = MediaPacer(); p.configure(bitrate: bitrate)
            var sent = 0; var completedAt = 100
            for tick in 0..<60 {
                p.advance(now: UInt64(tick) * 1_000_000)
                var turn = 0
                for _ in 0..<128 {
                    let payload = min(1036, size - sent); if payload == 0 { break }
                    guard p.take(bytes: payload + 64) else { break }
                    sent += payload; turn += payload + 64
                }
                XCTAssertLessThanOrEqual(Double(turn), p.peak * 0.005 + 1)
                if sent == size { completedAt = tick; break }
            }
            XCTAssertEqual(sent, size); XCTAssertLessThan(completedAt, 60)
        }
    }
    func testStoppingDatagramsDoesNotReportFailureToReplacementSession() throws {
        let queue = DispatchQueue(label: "test.datagram-cancel")
        let unexpected = expectation(description: "Intentional cancellation is not a transport failure")
        unexpected.isInverted = true
        let sender = DatagramSender(host: "127.0.0.1", port: 49161, queue: queue)
        queue.sync {
            sender.start(session: Data(repeating: 1, count: 16), token: Data(repeating: 2, count: 32), ready: {}, failure: { _ in unexpected.fulfill() })
            sender.stop()
        }
        wait(for: [unexpected], timeout: 0.5)
    }
    func testUSBSlowWriterKeepsOneOutstandingUnitAndRecoversAtIDR() {
        var now: UInt64 = 1
        var packets: [Data] = []; var completions: [() -> Void] = []; var requests = 0
        let sender = USBMediaSender(video: true, send: { data, done in packets.append(data); completions.append(done); return true }, now: { now })
        sender.dropped = { requests += 1 }
        sender.enqueue(unit(0, independent: true))
        for n in 1...20 { now += 1_000_000; sender.enqueue(unit(UInt64(n))) }
        XCTAssertEqual(packets.count, 1); XCTAssertEqual(completions.count, 1)
        XCTAssertEqual(requests, 1) // Backpressure asks for recovery without closing the socket.
        sender.enqueue(unit(21, independent: true)); completions.removeFirst()()
        XCTAssertEqual(packets.count, 2); XCTAssertEqual(packets[1].integer(at: 32, UInt64.self), 21)
        sender.stop(); completions.removeFirst()(); XCTAssertEqual(packets.count, 2)
    }
    func testWireBudgetLimitsIdrBurstsIncludingPacketHeaders() {
        var p = MediaPacer(); p.configure(bitrate: 24_000_000, wireBudgetMbps: 35)
        var sent = 0
        for tick in 0..<100 {
            p.advance(now: UInt64(tick) * 1_000_000)
            for _ in 0..<128 { if p.take(bytes: 1100) { sent += 1100 } else { break } }
        }
        XCTAssertLessThanOrEqual(Double(sent), 35_000_000.0 / 8 * 0.104 + 1100)
        XCTAssertGreaterThan(sent, 300_000)
    }
    func testBitrateChangePreservesMediaFormat() {
        let original = StreamConfig(); var changed = original
        changed.bitrate /= 2; XCTAssertTrue(changed.sameMediaFormat(as: original))
        changed.fps = 30; XCTAssertFalse(changed.sameMediaFormat(as: original))
    }
}
