import XCTest
import AVFoundation
@testable import TitanCam

final class LiveControlTests: XCTestCase {
    @MainActor
    func testStopCancelsPendingPermissionResult() async throws {
        let model = AppModel()
        var permission: CheckedContinuation<Bool, Never>?
        model.authorize = { await withCheckedContinuation { permission = $0 } }
        model.connect(usb: true)
        for _ in 0..<20 where permission == nil { await Task.yield() }
        XCTAssertNotNil(permission)
        model.stop()
        permission?.resume(returning: false)
        try await Task.sleep(nanoseconds: 50_000_000)
        XCTAssertNotEqual(model.status, "Permission needed", "A stopped request must not update UI or reconnect after authorization returns")
    }
    func testRequestCorrelationAndOptionalLegacyConfig() throws {
        let original = ControlMessage("Configure", session: String(repeating: "11", count: 16), body: [:], requestID: "capture-42")
        let received = try ControlMessage(data: original.data())
        XCTAssertEqual(received.requestID, "capture-42")
        let legacy = Data("""
        {"config_id":1,"profile":"balanced","width":1920,"height":1080,"fps":60,"bitrate":14000000,"codec":"h264","audio_codec":"opus","audio_channels":1,"audio_packet_ms":10,"playout_ms":60}
        """.utf8)
        let config = try JSONDecoder().decode(StreamConfig.self, from: legacy)
        XCTAssertTrue(config.valid)
        XCTAssertNil(config.camera_id)
        XCTAssertEqual(config.wifiBudgetMbps, 35)
        var changed = config
        changed.camera_id = "front"
        XCTAssertFalse(changed.sameMediaFormat(as: config))
        changed = config; changed.audio_data_source = 2
        XCTAssertFalse(changed.sameMediaFormat(as: config))
    }
    func testNonNativeAudioRateConvertsToTimestampedPCM() throws {
        let packetizer = AudioPacketizer()
        var config = StreamConfig()
        config.audio_codec = "pcm"; config.audio_packet_ms = 10
        try packetizer.configure(config, session: Data(repeating: 1, count: 16))
        var packets: [EncodedUnit] = []
        packetizer.output = { packets.append($0) }
        let format = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 44100, channels: 1, interleaved: false)!
        let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 441)!
        buffer.frameLength = 441
        for i in 0..<441 { buffer.floatChannelData![0][i] = 0.2 }
        var description: CMAudioFormatDescription?
        XCTAssertEqual(CMAudioFormatDescriptionCreate(allocator: kCFAllocatorDefault, asbd: format.streamDescription, layoutSize: 0, layout: nil, magicCookieSize: 0, magicCookie: nil, extensions: nil, formatDescriptionOut: &description), noErr)
        for n in 0..<20 {
            var sample: CMSampleBuffer?
            var timing = CMSampleTimingInfo(duration: CMTime(value: 1, timescale: 44100), presentationTimeStamp: CMTime(value: Int64(n*441), timescale: 44100), decodeTimeStamp: .invalid)
            XCTAssertEqual(CMSampleBufferCreate(allocator: kCFAllocatorDefault, dataBuffer: nil, dataReady: true, makeDataReadyCallback: nil, refcon: nil, formatDescription: description, sampleCount: 441, sampleTimingEntryCount: 1, sampleTimingArray: &timing, sampleSizeEntryCount: 0, sampleSizeArray: nil, sampleBufferOut: &sample), noErr)
            XCTAssertEqual(CMSampleBufferSetDataBufferFromAudioBufferList(sample!, blockBufferAllocator: kCFAllocatorDefault, blockBufferMemoryAllocator: kCFAllocatorDefault, flags: 0, bufferList: buffer.audioBufferList), noErr)
            packetizer.consume(sample!, pts: UInt64(n)*10_000_000)
        }
        XCTAssertGreaterThanOrEqual(packets.count, 18)
        XCTAssertTrue(packets.allSatisfy { $0.bytes.count == 960 && $0.duration == 10_000_000 })
        XCTAssertEqual(packets.map(\.sequence), Array(0..<UInt64(packets.count)))
    }
}
