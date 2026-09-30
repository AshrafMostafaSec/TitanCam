import Foundation
import AVFoundation
final class AudioPacketizer {
    private var encoder: OpaquePointer?
    private var ring: [Int16] = []
    private var nextPTS: UInt64 = 0
    private var sequence: UInt64 = 0
    private var config = StreamConfig()
    private var session = Data()
    var output: ((EncodedUnit) -> Void)?
    func configure(_ config: StreamConfig, session: Data) throws {
        if let encoder { opus_encoder_destroy(encoder) }; encoder = nil; ring.removeAll(keepingCapacity: true); sequence = 0; self.config = config; self.session = session
        if config.audio_codec == "opus" { var error: Int32 = 0; encoder = opus_encoder_create(48_000, Int32(config.audio_channels), config.audio_packet_ms == 5 ? OPUS_APPLICATION_RESTRICTED_LOWDELAY : OPUS_APPLICATION_AUDIO, &error); guard error == OPUS_OK, encoder != nil else { throw CameraError.unavailable("Opus encoder unavailable") }
            // Vararg CTL API is called from a fixed C bridge, because Swift cannot call C varargs.
            titan_opus_bitrate(encoder, config.profile == "saver" ? 64_000 : config.profile == "maximum" ? 256_000 : 96_000)
        }
    }
    func consume(_ sample: CMSampleBuffer, pts: UInt64) {
        guard let description = CMSampleBufferGetFormatDescription(sample), let asbd = CMAudioFormatDescriptionGetStreamBasicDescription(description) else { return }
        let format = asbd.pointee; guard abs(format.mSampleRate - 48_000) < 1, format.mFormatID == kAudioFormatLinearPCM else { return }
        var needed = 0; CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(sample, bufferListSizeNeededOut: &needed, bufferListOut: nil, bufferListSize: 0, blockBufferAllocator: nil, blockBufferMemoryAllocator: nil, flags: 0, blockBufferOut: nil)
        let memory = UnsafeMutableRawPointer.allocate(byteCount: max(needed, MemoryLayout<AudioBufferList>.size), alignment: MemoryLayout<AudioBufferList>.alignment); defer { memory.deallocate() }
        let list = memory.bindMemory(to: AudioBufferList.self, capacity: 1); var block: CMBlockBuffer?
        guard CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(sample, bufferListSizeNeededOut: nil, bufferListOut: list, bufferListSize: max(needed, MemoryLayout<AudioBufferList>.size), blockBufferAllocator: nil, blockBufferMemoryAllocator: nil, flags: 0, blockBufferOut: &block) == noErr else { return }
        let buffers = UnsafeMutableAudioBufferListPointer(list); let count = CMSampleBufferGetNumSamples(sample); let channels = Int(format.mChannelsPerFrame); let planar = format.mFormatFlags & kAudioFormatFlagIsNonInterleaved != 0; let floating = format.mFormatFlags & kAudioFormatFlagIsFloat != 0
        if ring.isEmpty { nextPTS = pts }
        // Ring cap is 100 ms. Route/sample-rate changes trigger configuration restart in CaptureEngine.
        guard ring.count + count * config.audio_channels <= 4_800 * config.audio_channels else { ring.removeAll(keepingCapacity: true); nextPTS = pts; return }
        for i in 0..<count { for c in 0..<config.audio_channels { let channel = min(c, channels - 1); let buffer = buffers[planar ? channel : 0]; guard let raw = buffer.mData else { continue }; let index = planar ? i : i * channels + channel
            if floating { let f = raw.assumingMemoryBound(to: Float.self)[index]; ring.append(Int16((max(-1, min(1, f)) * 32767).rounded())) } else if format.mBitsPerChannel == 16 { ring.append(raw.assumingMemoryBound(to: Int16.self)[index]) }
        } }
        let frames = 48 * config.audio_packet_ms; let size = frames * config.audio_channels
        while ring.count >= size { let pcm = Array(ring.prefix(size)); ring.removeFirst(size); var bytes: Data
            if let encoder { var packet = [UInt8](repeating: 0, count: 1_275); let n = pcm.withUnsafeBufferPointer { buffer in opus_encode(encoder, buffer.baseAddress, Int32(frames), &packet, 1_275) }; guard n > 0 else { continue }; bytes = Data(packet.prefix(Int(n))) } else { bytes = pcm.withUnsafeBytes { Data($0) } }
            output?(EncodedUnit(kind: encoder == nil ? 2 : 3, independent: false, session: session, config: config.config_id, sequence: sequence, pts: nextPTS, duration: UInt32(config.audio_packet_ms * 1_000_000), bytes: bytes)); sequence += 1; nextPTS += UInt64(config.audio_packet_ms * 1_000_000)
        }
    }
    deinit { if let encoder { opus_encoder_destroy(encoder) } }
}
