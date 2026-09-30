import Foundation
struct StreamConfig: Codable, Equatable {
    var config_id: UInt32 = 1
    var profile: String = "balanced"
    var width: Int = 1920
    var height: Int = 1080
    var fps: Int = 60
    var bitrate: Int = 14_000_000
    var codec: String = "h264"
    var audio_codec: String = "opus"
    var audio_channels: Int = 1
    var audio_packet_ms: Int = 10
    var playout_ms: Int = 10
    var valid: Bool { config_id > 0 && ["saver", "balanced", "maximum"].contains(profile) && (1...3840).contains(width) && (1...2160).contains(height) && (1...60).contains(fps) && (100_000...100_000_000).contains(bitrate) && ["h264", "hevc"].contains(codec) && ["pcm", "opus"].contains(audio_codec) && (1...2).contains(audio_channels) && [5, 10, 20].contains(audio_packet_ms) && (0...100).contains(playout_ms) }
}
struct ControlMessage {
    let type: String
    let session: String
    let epoch: UInt32
    var allowedBeforeHandshake: Bool { type == "Hello" }
    let body: [String: Any]
    init(_ type: String, session: String, body: [String: Any] = [:]) { self.type = type; self.session = session; self.body = body; self.epoch = 1 }
    init(data: Data) throws {
        guard data.count <= 65_536, let value = try JSONSerialization.jsonObject(with: data) as? [String: Any], value["version"] as? Int == 1, let type = value["type"] as? String, type.count <= 64, let session = value["session_id"] as? String, session.count == 32, let body = value["body"] as? [String: Any], ControlMessage.depth(body) <= 16 else { throw CameraError.protocolViolation("Invalid control message") }
        self.type = type; self.session = session; self.body = body; self.epoch = (value["transport_epoch"] as? NSNumber)?.uint32Value ?? 0
    }
    func data() throws -> Data { let data = try JSONSerialization.data(withJSONObject: ["version": 1, "type": type, "request_id": "0", "session_id": session, "transport_epoch": epoch, "body": body]); guard data.count <= 65_536 else { throw CameraError.protocolViolation("Control message too large") }; return data }
    private static func depth(_ value: Any) -> Int { if let dict = value as? [String: Any] { return 1 + (dict.values.map(depth).max() ?? 0) }; if let array = value as? [Any] { return 1 + (array.map(depth).max() ?? 0) }; return 0 }
}
enum CameraError: LocalizedError {
    case protocolViolation(String), unavailable(String), permission(String)
    var errorDescription: String? { switch self { case .protocolViolation(let s), .unavailable(let s), .permission(let s): return s } }
}
extension Data {
    init?(hex: String) { guard hex.count % 2 == 0 else { return nil }; var result = Data(); var i = hex.startIndex; while i < hex.endIndex { let j = hex.index(i, offsetBy: 2); guard let b = UInt8(hex[i..<j], radix: 16) else { return nil }; result.append(b); i = j }; self = result }
    var hex: String { map { String(format: "%02x", $0) }.joined() }
    mutating func appendBE<T: FixedWidthInteger>(_ value: T) { var n = value.bigEndian; Swift.withUnsafeBytes(of: &n) { append(contentsOf: $0) } }
    func integer<T: FixedWidthInteger>(at offset: Int, _: T.Type) -> T? { guard offset >= 0 && offset + MemoryLayout<T>.size <= count else { return nil }; return subdata(in: offset..<offset + MemoryLayout<T>.size).reduce(T.zero) { ($0 << 8) | T($1) } }
}
struct EncodedUnit {
    let kind: UInt8; let independent: Bool; let session: Data; let config: UInt32; let sequence: UInt64; let pts: UInt64; let duration: UInt32; let bytes: Data
    func packet(index: UInt16 = 0, count: UInt16 = 1, offset: Int = 0, length: Int? = nil) -> Data {
        var data = Data("TCAM".utf8); data.append(1); data.append(kind); data.appendBE(UInt16(independent ? 1 : 0)); data.append(session); data.appendBE(UInt32(1)); data.appendBE(config); data.appendBE(sequence); data.appendBE(pts); data.appendBE(duration); data.appendBE(UInt32(bytes.count)); data.appendBE(index); data.appendBE(count); data.appendBE(UInt32(offset)); data.append(bytes.subdata(in: offset..<offset + (length ?? bytes.count))); return data
    }
}
