import AVFoundation

/// Public device formats and microphone data sources, never guessed model tables.
enum CameraCatalog {
    static func devices() -> [AVCaptureDevice] {
        AVCaptureDevice.DiscoverySession(deviceTypes: [.builtInWideAngleCamera, .builtInUltraWideCamera, .builtInTrueDepthCamera], mediaType: .video, position: .unspecified).devices
    }
    static func camera(_ id: String?) -> AVCaptureDevice? {
        if let id { return devices().first { $0.uniqueID == id } }
        return AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: .back)
    }
    static func capabilities() -> [String: Any] {
        let cameras: [[String: Any]] = devices().prefix(8).map { device in
            var modes = Set<String>()
            var formats: [[String: Any]] = []
            for format in device.formats {
                let d = CMVideoFormatDescriptionGetDimensions(format.formatDescription)
                guard d.width <= 3840, d.height <= 2160 else { continue }
                for fps in [24, 25, 30, 50, 60] where format.videoSupportedFrameRateRanges.contains(where: { $0.minFrameRate <= Double(fps) && $0.maxFrameRate >= Double(fps) }) {
                    let key = "\(d.width)x\(d.height)@\(fps)"
                    if modes.insert(key).inserted { formats.append(["width":Int(d.width), "height":Int(d.height), "fps":fps]) }
                }
            }
            return ["id":device.uniqueID, "name":device.localizedName, "position":device.position == .front ? "front" : "back", "formats":Array(formats.prefix(128))]
        }
        let inputs: [[String: Any]] = (AVAudioSession.sharedInstance().availableInputs ?? []).prefix(16).map { port in
            let sources: [[String: Any]] = (port.dataSources ?? []).prefix(16).map { source in
                ["id":source.dataSourceID.uint32Value, "name":source.dataSourceName, "location":source.location?.rawValue ?? "", "orientation":source.orientation?.rawValue ?? ""]
            }
            return ["id":port.uid, "name":port.portName, "sources":sources]
        }
        return ["live_controls":true, "selective_repair":true, "cameras":cameras, "audio_inputs":inputs, "codecs":["h264", "hevc"], "color":"SDR", "max_fps":60]
    }
}
