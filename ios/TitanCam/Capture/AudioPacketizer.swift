import Foundation
import AVFoundation

final class AudioPacketizer {
    private var encoder: OpaquePointer?
    private var converter: AVAudioConverter?
    private var sourceRate: Double = 0
    private var sourceChannels = 0
    private var ring: [Int16] = []
    private var nextPTS: UInt64 = 0
    private var sequence: UInt64 = 0
    private var config = StreamConfig()
    private var session = Data()
    var output: ((EncodedUnit) -> Void)?
    var failure: ((String) -> Void)?
    func configure(_ config: StreamConfig, session: Data) throws {
        if let encoder { opus_encoder_destroy(encoder) }
        encoder = nil; converter = nil; sourceRate = 0; sourceChannels = 0
        ring.removeAll(keepingCapacity: true)
        if self.session != session { sequence = 0 }
        self.config = config; self.session = session
        if config.audio_codec == "opus" {
            var error: Int32 = 0
            encoder = opus_encoder_create(48_000, Int32(config.audio_channels), config.audio_packet_ms == 5 ? OPUS_APPLICATION_RESTRICTED_LOWDELAY : OPUS_APPLICATION_AUDIO, &error)
            guard error == OPUS_OK, encoder != nil else { throw CameraError.unavailable("Opus encoder unavailable") }
            titan_opus_bitrate(encoder, config.profile == "saver" ? 64_000 : config.profile == "maximum" ? 256_000 : 96_000)
        }
    }
    func consume(_ sample: CMSampleBuffer, pts: UInt64) {
        guard let description = CMSampleBufferGetFormatDescription(sample), let asbd = CMAudioFormatDescriptionGetStreamBasicDescription(description) else { return }
        let format = asbd.pointee
        let floating = format.mFormatFlags & kAudioFormatFlagIsFloat != 0
        guard format.mChannelsPerFrame > 0, (format.mBitsPerChannel == 16 || (format.mBitsPerChannel == 32 && floating)), format.mFormatID == kAudioFormatLinearPCM, (8_000...192_000).contains(format.mSampleRate) else { failure?("Unsupported microphone PCM format"); return }
        var needed = 0
        CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(sample, bufferListSizeNeededOut: &needed, bufferListOut: nil, bufferListSize: 0, blockBufferAllocator: nil, blockBufferMemoryAllocator: nil, flags: 0, blockBufferOut: nil)
        let size = max(needed, MemoryLayout<AudioBufferList>.size)
        let memory = UnsafeMutableRawPointer.allocate(byteCount: size, alignment: MemoryLayout<AudioBufferList>.alignment)
        defer { memory.deallocate() }
        let list = memory.bindMemory(to: AudioBufferList.self, capacity: 1)
        var block: CMBlockBuffer?
        guard CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(sample, bufferListSizeNeededOut: nil, bufferListOut: list, bufferListSize: size, blockBufferAllocator: nil, blockBufferMemoryAllocator: nil, flags: 0, blockBufferOut: &block) == noErr else { return }
        let buffers = UnsafeMutableAudioBufferListPointer(list)
        let count = CMSampleBufferGetNumSamples(sample)
        guard count > 0, count <= 8192 else { return }
        let channels = Int(format.mChannelsPerFrame)
        let planar = format.mFormatFlags & kAudioFormatFlagIsNonInterleaved != 0
        guard buffers.count >= (planar ? channels : 1), config.audio_channels <= channels else { failure?("Microphone channel layout changed; reconnect"); return }
        guard let sourceFormat = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: format.mSampleRate, channels: AVAudioChannelCount(config.audio_channels), interleaved: false), let input = AVAudioPCMBuffer(pcmFormat: sourceFormat, frameCapacity: AVAudioFrameCount(count)) else { return }
        input.frameLength = AVAudioFrameCount(count)
        for c in 0..<config.audio_channels {
            let buffer = buffers[planar ? c : 0]
            guard let raw = buffer.mData, Int(buffer.mDataByteSize) >= count * (planar ? 1 : channels) * Int(format.mBitsPerChannel / 8) else { return }
            for i in 0..<count {
                let index = planar ? i : i * channels + c
                input.floatChannelData![c][i] = floating ? raw.assumingMemoryBound(to: Float.self)[index] : Float(raw.assumingMemoryBound(to: Int16.self)[index]) / 32768
            }
        }
        guard let targetFormat = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 48_000, channels: AVAudioChannelCount(config.audio_channels), interleaved: false) else { return }
        if sourceRate != format.mSampleRate || sourceChannels != config.audio_channels {
            converter = AVAudioConverter(from: sourceFormat, to: targetFormat)
            sourceRate = format.mSampleRate; sourceChannels = config.audio_channels
        }
        guard let converter, let converted = AVAudioPCMBuffer(pcmFormat: targetFormat, frameCapacity: AVAudioFrameCount(ceil(Double(count) * 48_000 / format.mSampleRate) + 128)) else { return }
        var supplied = false
        var error: NSError?
        let status = converter.convert(to: converted, error: &error) { _, state in
            if supplied { state.pointee = .noDataNow; return nil }
            supplied = true; state.pointee = .haveData; return input
        }
        guard status != .error else { failure?(error?.localizedDescription ?? "Audio conversion failed"); return }
        let frames = Int(converted.frameLength)
        if ring.isEmpty { nextPTS = pts }
        guard ring.count + frames * config.audio_channels <= 4800 * config.audio_channels else { ring.removeAll(keepingCapacity: true); nextPTS = pts; return }
        for i in 0..<frames { for c in 0..<config.audio_channels {
            let value = converted.floatChannelData![c][i]
            ring.append(value.isFinite ? Int16((max(-1, min(1, value)) * 32767).rounded()) : 0)
        } }
        let packetFrames = 48 * config.audio_packet_ms
        let packetSamples = packetFrames * config.audio_channels
        while ring.count >= packetSamples {
            let pcm = Array(ring.prefix(packetSamples)); ring.removeFirst(packetSamples)
            var bytes: Data
            if let encoder {
                var packet = [UInt8](repeating: 0, count: 1275)
                let n = pcm.withUnsafeBufferPointer { opus_encode(encoder, $0.baseAddress!, Int32(packetFrames), &packet, 1275) }
                guard n > 0 else { failure?("Opus encoding failed (\(n))"); return }
                bytes = Data(packet.prefix(Int(n)))
            } else { bytes = pcm.withUnsafeBytes { Data($0) } }
            output?(EncodedUnit(kind: encoder == nil ? 2 : 3, independent: false, session: session, config: config.config_id, sequence: sequence, pts: nextPTS, duration: UInt32(config.audio_packet_ms * 1_000_000), bytes: bytes))
            sequence += 1; nextPTS += UInt64(config.audio_packet_ms * 1_000_000)
        }
    }
    deinit { if let encoder { opus_encoder_destroy(encoder) } }
}
