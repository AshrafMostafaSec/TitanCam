import SwiftUI
import AVFoundation
@MainActor
final class AppModel: ObservableObject {
    @Published var status = "Ready"
    @Published var detail = "Choose USB or paste the pairing link from your computer."
    @Published var stats: [String: String] = [:]
    var coordinator: StreamCoordinator?
    init() {
        do { let stream = try StreamCoordinator(); coordinator = stream; stream.update = { [weak self] status, detail, values in DispatchQueue.main.async { self?.status = status; self?.detail = detail; self?.stats = values.reduce(into: [:]) { $0[$1.key] = String(describing: $1.value) } } } }
        catch { status = "Setup required"; detail = error.localizedDescription }
    }
    func connect(usb: Bool, uri: String = "") {
        Task { guard await CaptureEngine.authorize() else { status = "Permission needed"; detail = "Allow Camera and Microphone in Settings, then try again."; return }; if usb { coordinator?.enableUSB() } else { coordinator?.startWiFi(uri.trimmingCharacters(in: .whitespacesAndNewlines)) } }
    }
}
struct ContentView: View {
    @StateObject private var model = AppModel()
    @State private var pairing = ""
    @State private var preview = false
    @Environment(\.scenePhase) private var phase
    var body: some View {
        ScrollView { VStack(alignment: .leading, spacing: 22) {
            HStack { Image(systemName: "video.fill").font(.largeTitle).foregroundStyle(.mint); VStack(alignment: .leading) { Text("TitanCam").font(.largeTitle.bold()); Text("iPhone → Linux • Development preview").foregroundStyle(.secondary) } }
            if preview, let session = model.coordinator?.capture.session { CameraPreview(session: session).frame(height: 210).clipShape(RoundedRectangle(cornerRadius: 20)) }
            VStack(alignment: .leading, spacing: 10) { Label(model.status, systemImage: model.status == "Streaming" ? "dot.radiowaves.left.and.right" : "circle").font(.title2.bold()); Text(model.detail).font(.system(.body, design: model.status == "USB ready" ? .monospaced : .default)).textSelection(.enabled) }.frame(maxWidth: .infinity, alignment: .leading).padding().background(.thinMaterial).clipShape(RoundedRectangle(cornerRadius: 18))
            Button { model.connect(usb: true) } label: { Label("Enable USB connection", systemImage: "cable.connector").frame(maxWidth: .infinity).padding(7) }.buttonStyle(.borderedProminent).tint(.mint)
            Text("Connect the cable, trust the computer, then use the certificate pin and one-time token above in the receiver. Pairing expires after two minutes.").font(.footnote).foregroundStyle(.secondary)
            VStack(alignment: .leading, spacing: 12) { Text("Wi-Fi pairing").font(.headline); TextField("Paste titancam://pair…", text: $pairing, axis: .vertical).textInputAutocapitalization(.never).autocorrectionDisabled().padding().background(.quaternary).clipShape(RoundedRectangle(cornerRadius: 12)); Button("Connect over Wi-Fi") { model.connect(usb: false, uri: pairing) }.buttonStyle(.bordered).disabled(pairing.isEmpty) }
            Toggle("Camera preview", isOn: $preview)
            Text("Choose saver, balanced or maximum on the Linux receiver. Maximum 4K60 is experimental; the app reports the effective camera settings and reduces load when hot.").font(.footnote).foregroundStyle(.secondary)
            if !model.stats.isEmpty { VStack(alignment: .leading, spacing: 8) { Text("Receiver health").font(.headline); ForEach(["video", "audio", "dropped", "expired", "decoder", "clock_rtt_ms"], id: \.self) { key in if let value = model.stats[key] { HStack { Text(key.replacingOccurrences(of: "_", with: " ")); Spacer(); Text(value).monospacedDigit() } } } }.font(.caption).padding().background(.quaternary).clipShape(RoundedRectangle(cornerRadius: 16)) }
            Button("Stop streaming", role: .destructive) { model.coordinator?.stop() }.buttonStyle(.bordered)
            Text("Keep this app open and the phone unlocked. Displayed counters come from the receiver; they do not measure glass-to-glass latency.").font(.footnote).foregroundStyle(.secondary)
        }.padding(24) }.preferredColorScheme(.dark).onChange(of: phase) { phase in if phase != .active { model.coordinator?.stop() }; UIApplication.shared.isIdleTimerDisabled = phase == .active }.onOpenURL { url in if url.scheme == "titancam" { pairing = url.absoluteString } }.onAppear { UIApplication.shared.isIdleTimerDisabled = true }
    }
}
private struct CameraPreview: UIViewRepresentable {
    let session: AVCaptureSession
    final class Preview: UIView { override class var layerClass: AnyClass { AVCaptureVideoPreviewLayer.self }; var video: AVCaptureVideoPreviewLayer { layer as! AVCaptureVideoPreviewLayer }; override func layoutSubviews() { super.layoutSubviews(); video.connection?.videoOrientation = .landscapeRight } }
    func makeUIView(context: Context) -> Preview { let view = Preview(); view.video.session = session; view.video.videoGravity = .resizeAspectFill; return view }
    func updateUIView(_ view: Preview, context: Context) {}
}
