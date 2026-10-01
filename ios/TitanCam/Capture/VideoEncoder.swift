import Foundation
import AVFoundation
import VideoToolbox
final class VideoEncoder {
    private final class Ticket { let generation: UInt64; let independent: Bool; init(_ generation: UInt64, independent: Bool) { self.generation = generation; self.independent = independent } }
    private var generation: UInt64 = 0
    private var colorApplied = false
    private var captured: UInt64 = 0
    private var encodedCount: UInt64 = 0
    private var captureDrops: UInt64 = 0
    private var skipped: UInt64 = 0
    private var keyframes: UInt64 = 0
    private var lastRequestedIDR: UInt64 = 0
    private var encoder: VTCompressionSession?
    private var inFlight = 0
    private var sequence: UInt64 = 0
    private var forceIDR = true
    private var lastIDR: UInt64 = 0
    private var config = StreamConfig()
    private var sessionID = Data()
    private(set) var hardwareEvidence = ""
    private(set) var hardwareQueryStatus: OSStatus = noErr
    let queue = DispatchQueue(label: "titancam.video", qos: .userInteractive)
    var output: ((EncodedUnit) -> Void)?
    var failure: ((String) -> Void)?
    func configure(_ config: StreamConfig, session: Data) throws {
        generation += 1; colorApplied = false; self.config = config; if self.sessionID != session { sequence = 0; captured = 0; encodedCount = 0; captureDrops = 0; skipped = 0; keyframes = 0; lastRequestedIDR = 0 }; self.sessionID = session; forceIDR = true; lastIDR = 0
        if let encoder { VTCompressionSessionInvalidate(encoder) }; encoder = nil; inFlight = 0
        var specification: [CFString: Any] = [kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder: true, kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder: true]
        if config.codec == "h264" { specification[kVTVideoEncoderSpecification_EnableLowLatencyRateControl] = true }
        let callback: VTCompressionOutputCallback = { context, source, status, flags, sample in
            guard let context else { return }; let owner = Unmanaged<VideoEncoder>.fromOpaque(context).takeUnretainedValue()
            guard let source else { return }; let ticket = Unmanaged<Ticket>.fromOpaque(source).takeRetainedValue()
            owner.queue.async { guard owner.generation == ticket.generation else { return }; owner.inFlight = max(0, owner.inFlight - 1); if status == noErr, !flags.contains(.frameDropped), let sample { owner.encoded(sample) } else { owner.skipped += 1; if status != noErr || ticket.independent { owner.forceIDR = true } } }
        }
        var created: VTCompressionSession?
        let status = VTCompressionSessionCreate(allocator: kCFAllocatorDefault, width: Int32(config.width), height: Int32(config.height), codecType: config.codec == "hevc" ? kCMVideoCodecType_HEVC : kCMVideoCodecType_H264, encoderSpecification: specification as CFDictionary, imageBufferAttributes: nil, compressedDataAllocator: nil, outputCallback: callback, refcon: Unmanaged.passUnretained(self).toOpaque(), compressionSessionOut: &created)
        guard status == noErr, let created else { throw CameraError.unavailable("Hardware encoder creation failed (\(status))") }; encoder = created
        for (key, value) in [(kVTCompressionPropertyKey_RealTime, true as Any), (kVTCompressionPropertyKey_AllowFrameReordering, false as Any), (kVTCompressionPropertyKey_AverageBitRate, config.bitrate as Any), (kVTCompressionPropertyKey_ExpectedFrameRate, config.fps as Any)] {
            let result = VTSessionSetProperty(created, key: key, value: value as CFTypeRef); guard result == noErr else { throw CameraError.unavailable("Encoder setting rejected: \(key) (\(result))") }
        }
        if config.codec == "hevc" {
            let status = VTSessionSetProperty(created, key: kVTCompressionPropertyKey_ProfileLevel, value: kVTProfileLevel_HEVC_Main_AutoLevel)
            guard status == noErr else { throw CameraError.unavailable("HEVC Main profile rejected (\(status))") }
        }
        if config.codec == "h264" { let status = VTSessionSetProperty(created, key: kVTCompressionPropertyKey_ProfileLevel, value: kVTProfileLevel_H264_High_AutoLevel); guard status == noErr else { throw CameraError.unavailable("H.264 profile rejected (\(status))") } }
        guard VTCompressionSessionPrepareToEncodeFrames(created) == noErr else { throw CameraError.unavailable("Encoder preparation failed") }
        var hardware: CFTypeRef?
        hardwareQueryStatus = VTSessionCopyProperty(created, key: kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder, allocator: nil, valueOut: &hardware)
        hardwareEvidence = try HardwareEncoderPolicy.evidence(requiredHardware: true, queryStatus: hardwareQueryStatus, value: hardware)
        NSLog("TitanCam encoder hardware evidence=%@ query_status=%d", hardwareEvidence, hardwareQueryStatus)
    }
    func setBitrate(_ bitrate: Int) throws {
        guard let encoder else { throw CameraError.unavailable("Encoder is not configured") }
        let status = VTSessionSetProperty(encoder, key: kVTCompressionPropertyKey_AverageBitRate, value: NSNumber(value: bitrate))
        guard status == noErr else { throw CameraError.unavailable("Bitrate update rejected (\(status))") }
        config.bitrate = bitrate
    }
    func captureDropped() { captureDrops += 1 }
    func statistics() -> [String: Any] { ["captured":captured,"encoded":encodedCount,"capture_dropped":captureDrops,"encoder_skipped":skipped,"keyframes":keyframes] }
    func encode(_ sample: CMSampleBuffer, pts: UInt64) {
        captured += 1
        guard let encoder, let pixel = CMSampleBufferGetImageBuffer(sample) else { return }
        guard CVPixelBufferGetWidth(pixel) == config.width, CVPixelBufferGetHeight(pixel) == config.height else {
            failure?("Capture delivered dimensions do not match the effective encoder format")
            return
        }
        if !colorApplied {
            for (source, destination) in [(kCVImageBufferColorPrimariesKey, kVTCompressionPropertyKey_ColorPrimaries), (kCVImageBufferTransferFunctionKey, kVTCompressionPropertyKey_TransferFunction), (kCVImageBufferYCbCrMatrixKey, kVTCompressionPropertyKey_YCbCrMatrix)] {
                if let value = CVBufferCopyAttachment(pixel, source, nil) { let status = VTSessionSetProperty(encoder, key: destination, value: value); if status != noErr { failure?("Color metadata rejected (\(status))"); return } }
            }; colorApplied = true
        }
        guard inFlight < 2 else { skipped += 1; return }
        let independent = forceIDR || pts >= lastIDR + 1_000_000_000
        let props: [CFString: Any] = independent ? [kVTEncodeFrameOptionKey_ForceKeyFrame: true] : [:]
        if independent { lastIDR = pts; forceIDR = false }
        inFlight += 1
        let ticket = Unmanaged.passRetained(Ticket(generation, independent: independent)).toOpaque()
        let result = VTCompressionSessionEncodeFrame(encoder, imageBuffer: pixel, presentationTimeStamp: CMTime(value: Int64(pts), timescale: 1_000_000_000), duration: CMTime(value: 1, timescale: CMTimeScale(config.fps)), frameProperties: props as CFDictionary, sourceFrameRefcon: ticket, infoFlagsOut: nil)
        if result != noErr { Unmanaged<Ticket>.fromOpaque(ticket).release(); inFlight -= 1; forceIDR = true; failure?("Encoder failed (\(result))") }
    }
    func requestIDR() { queue.async { let now = CaptureEngine.hostTime; if self.lastRequestedIDR == 0 || now - self.lastRequestedIDR >= 250_000_000 { self.lastRequestedIDR = now; self.forceIDR = true } } }
    private func encoded(_ sample: CMSampleBuffer) {
        guard CMSampleBufferDataIsReady(sample), let block = CMSampleBufferGetDataBuffer(sample), let format = CMSampleBufferGetFormatDescription(sample) else { return }
        let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: false) as? [[CFString: Any]]
        let independent = !(attachments?.first?[kCMSampleAttachmentKey_NotSync] as? Bool ?? false)
        var bytes = Data(); var lengthSize: Int32 = 4
        if independent {
            var count = 0
            if config.codec == "hevc" { _ = CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(format, parameterSetIndex: 0, parameterSetPointerOut: nil, parameterSetSizeOut: nil, parameterSetCountOut: &count, nalUnitHeaderLengthOut: &lengthSize) }
            else { _ = CMVideoFormatDescriptionGetH264ParameterSetAtIndex(format, parameterSetIndex: 0, parameterSetPointerOut: nil, parameterSetSizeOut: nil, parameterSetCountOut: &count, nalUnitHeaderLengthOut: &lengthSize) }
            for i in 0..<count { var pointer: UnsafePointer<UInt8>?; var size = 0
                let result = config.codec == "hevc" ? CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(format, parameterSetIndex: i, parameterSetPointerOut: &pointer, parameterSetSizeOut: &size, parameterSetCountOut: nil, nalUnitHeaderLengthOut: nil) : CMVideoFormatDescriptionGetH264ParameterSetAtIndex(format, parameterSetIndex: i, parameterSetPointerOut: &pointer, parameterSetSizeOut: &size, parameterSetCountOut: nil, nalUnitHeaderLengthOut: nil)
                if result == noErr, let pointer { bytes.append(contentsOf: [0, 0, 0, 1]); bytes.append(pointer, count: size) }
            }
        }
        let size = CMBlockBufferGetDataLength(block); guard size < 8 * 1024 * 1024 else { forceIDR = true; return }
        var encoded = Data(count: size); let copied = encoded.withUnsafeMutableBytes { CMBlockBufferCopyDataBytes(block, atOffset: 0, dataLength: size, destination: $0.baseAddress!) }; guard copied == noErr else { return }
        var offset = 0; let header = Int(lengthSize); guard (1...4).contains(header) else { return }
        while offset + header <= encoded.count { let n = encoded.subdata(in: offset..<offset + header).reduce(0) { ($0 << 8) | Int($1) }; offset += header; guard n > 0 && offset + n <= encoded.count else { forceIDR = true; return }; bytes.append(contentsOf: [0, 0, 0, 1]); bytes.append(encoded.subdata(in: offset..<offset + n)); offset += n }
        guard offset == encoded.count else { forceIDR = true; return }
        let pts = CMTimeConvertScale(CMSampleBufferGetPresentationTimeStamp(sample), timescale: 1_000_000_000, method: .default).value
        encodedCount += 1; if independent { keyframes += 1 }
        output?(EncodedUnit(kind: 1, independent: independent, session: sessionID, config: config.config_id, sequence: sequence, pts: UInt64(max(0, pts)), duration: UInt32(1_000_000_000 / config.fps), bytes: bytes)); sequence += 1
    }
    func stop() { queue.sync { if let encoder = self.encoder { VTCompressionSessionCompleteFrames(encoder, untilPresentationTimeStamp: .invalid); VTCompressionSessionInvalidate(encoder) }; self.encoder = nil } }
    deinit { if let encoder { VTCompressionSessionInvalidate(encoder) } }
}
