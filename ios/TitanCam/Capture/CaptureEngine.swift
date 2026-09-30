import Foundation
import AVFoundation
final class CaptureEngine: NSObject, AVCaptureVideoDataOutputSampleBufferDelegate, AVCaptureAudioDataOutputSampleBufferDelegate {
    let session = AVCaptureSession()
    let encoder = VideoEncoder()
    let audio = AudioPacketizer()
    let queue = DispatchQueue(label: "titancam.capture", qos: .userInitiated)
    let audioQueue = DispatchQueue(label: "titancam.audio", qos: .userInteractive)
    var output: ((EncodedUnit) -> Void)?
    var failure: ((String) -> Void)?
    private(set) var epoch: UInt64 = 0
    private(set) var effective = StreamConfig()
    private var camera: AVCaptureDevice?
    private var configured = false
    private var observers: [NSObjectProtocol] = []
    static var hostTime: UInt64 { let t = CMTimeConvertScale(CMClockGetTime(CMClockGetHostTimeClock()), timescale: 1_000_000_000, method: .default); return UInt64(max(0, t.value)) }
    override init() {
        super.init(); encoder.output = { [weak self] in self?.output?($0) }; audio.output = { [weak self] in self?.output?($0) }; encoder.failure = { [weak self] in self?.failure?($0) }
        for name in [AVCaptureSession.wasInterruptedNotification, AVCaptureSession.runtimeErrorNotification, AVAudioSession.interruptionNotification, AVAudioSession.routeChangeNotification] { observers.append(NotificationCenter.default.addObserver(forName: name, object: nil, queue: nil) { [weak self] _ in self?.failure?("Camera or microphone interrupted. Reconnect after returning to the app.") }) }
    }
    static func authorize() async -> Bool { let camera = await AVCaptureDevice.requestAccess(for: .video); let microphone = await AVCaptureDevice.requestAccess(for: .audio); return camera && microphone }
    func configure(_ requested: StreamConfig, sessionID: Data, completion: @escaping (Result<StreamConfig, Error>) -> Void) {
        queue.async { do {
            guard requested.valid else { throw CameraError.protocolViolation("Unsupported configuration") }
            let running = self.session.isRunning; if running { self.session.stopRunning() }; self.encoder.stop()
            var config = requested
            let device = self.camera ?? AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: .back)
            guard let device else { throw CameraError.unavailable("Camera unavailable") }; self.camera = device
            let match: (Int, Int, Int) -> AVCaptureDevice.Format? = { w, h, fps in device.formats.first { format in let dimensions = CMVideoFormatDescriptionGetDimensions(format.formatDescription); return Int(dimensions.width) == w && Int(dimensions.height) == h && format.videoSupportedFrameRateRanges.contains { $0.minFrameRate <= Double(fps) && $0.maxFrameRate >= Double(fps) } } }
            var format = match(config.width, config.height, config.fps)
            if format == nil { config.width = 1920; config.height = 1080; config.fps = min(30, config.fps); config.profile = "balanced"; config.bitrate = min(config.bitrate, 14_000_000); format = match(config.width, config.height, config.fps) }
            guard let format else { throw CameraError.unavailable("Requested camera format unavailable") }
            self.session.beginConfiguration(); defer { self.session.commitConfiguration() }
            if !self.configured {
                self.session.automaticallyConfiguresApplicationAudioSession = false
                let cam = try AVCaptureDeviceInput(device: device); guard self.session.canAddInput(cam) else { throw CameraError.unavailable("Camera input unavailable") }; self.session.addInput(cam)
                guard let microphone = AVCaptureDevice.default(for: .audio) else { throw CameraError.unavailable("Microphone unavailable") }; let mic = try AVCaptureDeviceInput(device: microphone); guard self.session.canAddInput(mic) else { throw CameraError.unavailable("Microphone input unavailable") }; self.session.addInput(mic)
                let video = AVCaptureVideoDataOutput(); video.alwaysDiscardsLateVideoFrames = true; video.videoSettings = [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange]; video.setSampleBufferDelegate(self, queue: self.encoder.queue); guard self.session.canAddOutput(video) else { throw CameraError.unavailable("Video output unavailable") }; self.session.addOutput(video)
                if let connection = video.connection(with: .video) { connection.videoOrientation = .landscapeRight; if connection.isVideoStabilizationSupported { connection.preferredVideoStabilizationMode = .off } }
                let audio = AVCaptureAudioDataOutput(); audio.audioSettings = [AVFormatIDKey: kAudioFormatLinearPCM, AVSampleRateKey: 48_000, AVNumberOfChannelsKey: 1, AVLinearPCMBitDepthKey: 16, AVLinearPCMIsFloatKey: false, AVLinearPCMIsNonInterleaved: false]; audio.setSampleBufferDelegate(self, queue: self.audioQueue); guard self.session.canAddOutput(audio) else { throw CameraError.unavailable("Audio output unavailable") }; self.session.addOutput(audio); self.configured = true
            }
            try device.lockForConfiguration(); device.activeFormat = format; device.activeVideoMinFrameDuration = CMTime(value: 1, timescale: CMTimeScale(config.fps)); device.activeVideoMaxFrameDuration = CMTime(value: 1, timescale: CMTimeScale(config.fps)); if device.isVideoHDREnabled { device.isVideoHDREnabled = false }; device.unlockForConfiguration()
            let audioSession = AVAudioSession.sharedInstance(); try audioSession.setCategory(.record, mode: .measurement); try audioSession.setPreferredSampleRate(48_000); try audioSession.setPreferredIOBufferDuration(0.005); try audioSession.setActive(true)
            guard abs(audioSession.sampleRate - 48_000) < 1 else { throw CameraError.unavailable("This audio route needs a 48 kHz converter; choose the built-in microphone.") }
            config.audio_channels = 1 // Actual mono route is reported; never label duplicated mono as stereo.
            try self.encoder.queue.sync { try self.encoder.configure(config, session: sessionID) }; try self.audioQueue.sync { try self.audio.configure(config, session: sessionID) }
            self.effective = config; if !running { self.epoch = Self.hostTime }; completion(.success(config))
            if running { self.queue.async { self.session.startRunning() } }
        } catch { completion(.failure(error)) } }
    }
    func start() { queue.async { self.session.startRunning() } }
    func stop() { queue.async { if self.session.isRunning { self.session.stopRunning() }; self.encoder.stop(); try? AVAudioSession.sharedInstance().setActive(false) } }
    func requestIDR() { encoder.requestIDR() }
    func captureOutput(_ output: AVCaptureOutput, didOutput sample: CMSampleBuffer, from connection: AVCaptureConnection) {
        let pts = CMSampleBufferGetPresentationTimeStamp(sample)
        let host = session.masterClock.map { CMSyncConvertTime(pts, from: $0, to: CMClockGetHostTimeClock()) } ?? pts
        let absolute = UInt64(max(0, CMTimeConvertScale(host, timescale: 1_000_000_000, method: .default).value)); guard absolute >= epoch else { return }
        if output is AVCaptureVideoDataOutput { encoder.encode(sample, pts: absolute - epoch) } else { audio.consume(sample, pts: absolute - epoch) }
    }
    deinit { for observer in observers { NotificationCenter.default.removeObserver(observer) } }
}
