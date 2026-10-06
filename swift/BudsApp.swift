import SwiftUI
import AppKit

// MARK: - State from the Rust daemon

struct CellState: Codable, Equatable { var percent: Int; var charging: Bool }
struct BatteryState: Codable, Equatable { var left: CellState?; var right: CellState?; var `case`: CellState? }
struct BudsState: Codable, Equatable {
    var connected = false
    var mode: String?
    var battery: BatteryState?
    var eq: String?
    var spatial: String?
}

final class Daemon: ObservableObject {
    @Published var state = BudsState()
    private var proc: Process?
    private var stdin: FileHandle?
    private var buffer = Data()

    init() { start() }

    func start() {
        guard let exe = Bundle.main.executableURL?.deletingLastPathComponent().appendingPathComponent("buds-daemon") else { return }
        let p = Process()
        p.executableURL = exe
        let out = Pipe(), inp = Pipe()
        p.standardOutput = out
        p.standardInput = inp
        out.fileHandleForReading.readabilityHandler = { [weak self] h in
            let d = h.availableData
            guard !d.isEmpty else { return }
            DispatchQueue.main.async { self?.ingest(d) }
        }
        try? p.run()
        proc = p
        stdin = inp.fileHandleForWriting
    }

    private func ingest(_ d: Data) {
        buffer.append(d)
        while let nl = buffer.firstIndex(of: 10) {
            let line = buffer.subdata(in: buffer.startIndex..<nl)
            buffer.removeSubrange(buffer.startIndex...nl)
            if let s = try? JSONDecoder().decode(BudsState.self, from: line) { state = s }
        }
    }

    func send(_ cmd: String) {
        try? stdin?.write(contentsOf: Data((cmd + "\n").utf8))
    }

    func setMode(_ m: String) { state.mode = m; send("mode \(m)") }
    func setEq(_ e: String) { state.eq = e; send("eq \(e)") }
    func setSpatial(_ s: String) { state.spatial = s; send("spatial \(s)") }
}

// MARK: - Views

struct BatteryRing: View {
    let title: String
    let symbol: String
    let cell: CellState?

    var body: some View {
        VStack(spacing: 6) {
            ZStack {
                Circle().stroke(.quaternary, lineWidth: 5)
                if let c = cell {
                    Circle()
                        .trim(from: 0, to: CGFloat(c.percent) / 100)
                        .stroke(c.percent <= 20 && !c.charging ? Color.red : Color.green,
                                style: StrokeStyle(lineWidth: 5, lineCap: .round))
                        .rotationEffect(.degrees(-90))
                }
                Image(systemName: symbol).font(.system(size: 17)).foregroundStyle(.primary)
                if cell?.charging == true {
                    Image(systemName: "bolt.fill").font(.system(size: 9)).foregroundStyle(.green)
                        .offset(y: -17).opacity(0)
                }
            }
            .frame(width: 52, height: 52)
            HStack(spacing: 2) {
                if cell?.charging == true { Image(systemName: "bolt.fill").font(.system(size: 9)).foregroundStyle(.green) }
                Text(cell.map { "\($0.percent)%" } ?? "–").font(.system(size: 12, weight: .medium)).monospacedDigit()
            }
            Text(title).font(.system(size: 11)).foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity)
    }
}

struct ModeOption: Identifiable {
    let id: String   // "off", "transparency", "adaptive", "nc"
    let label: String
    let symbol: String
}

let modeOptions = [
    ModeOption(id: "off", label: "Off", symbol: "person.crop.circle"),
    ModeOption(id: "transparency", label: "Trans-\nparency", symbol: "person.wave.2"),
    ModeOption(id: "adaptive", label: "Adaptive", symbol: "sparkles"),
    ModeOption(id: "nc", label: "Noise\nCancellation", symbol: "person.crop.circle.fill"),
]

func group(of mode: String?) -> String? {
    switch mode {
    case "high", "medium", "low": return "nc"
    default: return mode
    }
}

struct Thumb: View {
    var body: some View {
        if #available(macOS 26, *) {
            Color.clear.glassEffect(.regular.interactive(), in: .rect(cornerRadius: 10))
        } else {
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(Color(nsColor: .controlBackgroundColor))
                .shadow(color: .black.opacity(0.15), radius: 2, y: 1)
        }
    }
}

struct SegmentedIcons: View {
    let options: [ModeOption]
    let selected: String?
    let enabled: Bool
    let onSelect: (String) -> Void
    @State private var dragIndex: Int?
    @State private var pressed = false

    private let pad: CGFloat = 3

    var body: some View {
        GeometryReader { geo in
            let segW = (geo.size.width - pad * 2) / CGFloat(options.count)
            let selIndex = options.firstIndex { $0.id == selected }
            let shown = dragIndex ?? selIndex
            ZStack(alignment: .leading) {
                RoundedRectangle(cornerRadius: 13, style: .continuous).fill(.quaternary.opacity(0.6))
                if let i = shown {
                    Thumb()
                        .frame(width: segW, height: geo.size.height - pad * 2)
                        .scaleEffect(pressed ? 1.06 : 1)
                        .offset(x: pad + CGFloat(i) * segW)
                        .animation(.spring(response: 0.3, dampingFraction: 0.78), value: i)
                        .animation(.spring(response: 0.25, dampingFraction: 0.7), value: pressed)
                }
                HStack(spacing: 0) {
                    ForEach(Array(options.enumerated()), id: \.element.id) { idx, o in
                        let on = idx == shown
                        VStack(spacing: 5) {
                            Image(systemName: o.symbol).font(.system(size: 22)).frame(width: 34, height: 30, alignment: .center)
                            Text(o.label)
                                .font(.system(size: 10.5, weight: .medium))
                                .foregroundStyle(on ? Color.primary : Color.primary.opacity(0.75))
                                .minimumScaleFactor(0.85)
                                .multilineTextAlignment(.center)
                                .lineLimit(2)
                                .frame(height: 28, alignment: .top)
                        }
                        .padding(.vertical, 8)
                        .frame(width: segW, height: geo.size.height - pad * 2)
                    }
                }
                .padding(.horizontal, pad)
                .allowsHitTesting(false)
            }
            .contentShape(Rectangle())
            .gesture(
                DragGesture(minimumDistance: 0)
                    .onChanged { v in
                        pressed = true
                        let i = Int(((v.location.x - pad) / segW).rounded(.down))
                        dragIndex = min(max(i, 0), options.count - 1)
                    }
                    .onEnded { _ in
                        pressed = false
                        if let i = dragIndex, options[i].id != selected { onSelect(options[i].id) }
                        dragIndex = nil
                    }
            )
        }
        .frame(height: 84)
        .opacity(enabled ? 1 : 0.45)
        .disabled(!enabled)
    }
}

struct Section<Content: View>: View {
    let title: String
    @ViewBuilder var content: Content
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title.uppercased()).font(.system(size: 11, weight: .semibold)).foregroundStyle(.secondary)
            content
        }
    }
}

struct PickerRow: View {
    let title: String
    let options: [(String, String)]
    let selected: String?
    let enabled: Bool
    let onSelect: (String) -> Void

    var body: some View {
        HStack {
            Text(title).font(.system(size: 13))
            Spacer()
            Menu {
                ForEach(options, id: \.0) { id, label in
                    Button { onSelect(id) } label: {
                        if id == selected { Label(label, systemImage: "checkmark") } else { Text(label) }
                    }
                }
            } label: {
                Text(options.first { $0.0 == selected }?.1 ?? "–").font(.system(size: 13))
            }
            .menuStyle(.button)
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .fixedSize()
        }
        .padding(.horizontal, 12).padding(.vertical, 9)
        .opacity(enabled ? 1 : 0.45).disabled(!enabled)
    }
}

let eqOptions = [("balanced", "Balanced"), ("bold", "Bold"), ("serenade", "Serenade"), ("bassboost", "Bass boost"), ("dynaudio", "Dynaudio")]
let spatialOptions = [("off", "Off", "person.fill"), ("fixed", "Fixed", "person.wave.2.fill"), ("headtracked", "Head Tracked", "person.line.dotted.person.fill")]

struct SpatialRow: View {
    let symbol: String
    let label: String
    let selected: Bool
    let enabled: Bool
    let action: () -> Void
    @State private var hover = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 10) {
                Image(systemName: "checkmark").font(.system(size: 12, weight: .semibold))
                    .frame(width: 16).opacity(selected ? 1 : 0)
                Image(systemName: symbol).font(.system(size: 15)).frame(width: 26)
                Text(label).font(.system(size: 13, weight: selected ? .medium : .regular))
                Spacer()
            }
            .padding(.horizontal, 12).padding(.vertical, 8)
            .background(hover && enabled ? Color.primary.opacity(0.08) : .clear)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hover = $0 }
        .opacity(enabled ? 1 : 0.45).disabled(!enabled)
    }
}
let ncLevels = [("high", "High"), ("medium", "Medium"), ("low", "Low")]

struct Panel: View {
    @ObservedObject var daemon: Daemon

    var body: some View {
        let s = daemon.state
        let grp = group(of: s.mode)
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 10) {
                Image(systemName: "earbuds").font(.system(size: 20))
                VStack(alignment: .leading, spacing: 1) {
                    Text("OnePlus Buds Pro 3").font(.system(size: 14, weight: .semibold))
                    Text(s.connected ? "Connected" : "Not connected")
                        .font(.system(size: 11)).foregroundStyle(.secondary)
                }
                Spacer()
                if !s.connected {
                    Button("Reconnect") { daemon.send("reconnect") }.controlSize(.small)
                }
            }

            HStack(spacing: 4) {
                BatteryRing(title: "Left", symbol: "earbuds", cell: s.connected ? s.battery?.left : nil)
                BatteryRing(title: "Right", symbol: "earbuds", cell: s.connected ? s.battery?.right : nil)
                BatteryRing(title: "Case", symbol: "earbuds.case", cell: s.connected ? s.battery?.case : nil)
            }

            Section(title: "Noise Control") {
                SegmentedIcons(options: modeOptions, selected: grp, enabled: s.connected) { id in
                    daemon.setMode(id == "nc" ? "high" : id)
                }
                if grp == "nc" {
                    Picker("", selection: Binding(get: { s.mode ?? "high" }, set: { daemon.setMode($0) })) {
                        ForEach(ncLevels, id: \.0) { Text($0.1).tag($0.0) }
                    }
                    .pickerStyle(.segmented).labelsHidden().disabled(!s.connected)
                }
            }

            VStack(spacing: 0) {
                PickerRow(title: "Equalizer", options: eqOptions, selected: s.eq, enabled: s.connected) { daemon.setEq($0) }
            }
            .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(.quaternary.opacity(0.5)))

            Section(title: "Spatial Audio") {
                VStack(spacing: 0) {
                    ForEach(Array(spatialOptions.enumerated()), id: \.element.0) { i, o in
                        if i > 0 { Divider().padding(.leading, 50) }
                        SpatialRow(symbol: o.2, label: o.1, selected: s.spatial == o.0, enabled: s.connected) {
                            daemon.setSpatial(o.0)
                        }
                    }
                }
                .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(.quaternary.opacity(0.5)))
            }

            HStack {
                Spacer()
                Button("Quit") { NSApplication.shared.terminate(nil) }
                    .buttonStyle(.plain).font(.system(size: 12)).foregroundStyle(.secondary)
            }
        }
        .padding(16)
        .frame(width: 340)
    }
}

@main
struct BudsApp: App {
    @StateObject private var daemon = Daemon()

    var body: some Scene {
        MenuBarExtra {
            Panel(daemon: daemon)
        } label: {
            Image(systemName: daemon.state.connected ? "earbuds" : "earbuds")
                .opacity(daemon.state.connected ? 1 : 0.5)
        }
        .menuBarExtraStyle(.window)
    }
}
