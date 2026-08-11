// MeetingAppResolver.swift
//
// Finds every running process that belongs to a meeting app's bundle.
// Multi-process apps (Teams Electron/WebView helpers, Zoom Frameworks hosts)
// often put call audio in a child process: tapping only the shell PID
// silently captures nothing, so every PID under the bundle must be included.
//
// Validated for Teams and Zoom in spike/capture-spike before production use.

import Foundation

enum MeetingAppResolver {
    /// Runs `ps -eo pid=,comm=` and returns (pid, executablePath) pairs for
    /// every process whose executable matches the app's path fragments.
    static func discoverProcesses(for app: BlueEarMeetingApp) -> [(pid: pid_t, path: String)] {
        guard let descriptor = MeetingAppCatalog.descriptor(for: app) else { return [] }

        let task = Process()
        task.executableURL = URL(fileURLWithPath: "/bin/ps")
        task.arguments = ["-eo", "pid=,comm="]
        let pipe = Pipe()
        task.standardOutput = pipe
        task.standardError = FileHandle.nullDevice

        do {
            try task.run()
        } catch {
            return []
        }

        // IMPORTANT: drain the pipe before waiting for exit. `ps` output can
        // exceed the kernel pipe buffer; waiting for exit first deadlocks
        // (ps blocks on write, we block on wait). Discovered and fixed
        // during the capture spike.
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        task.waitUntilExit()

        guard let output = String(data: data, encoding: .utf8) else { return [] }

        var results: [(pid_t, String)] = []
        for rawLine in output.split(separator: "\n") {
            let trimmed = rawLine.trimmingCharacters(in: .whitespaces)
            guard let spaceIdx = trimmed.firstIndex(of: " ") else { continue }
            let pidString = trimmed[trimmed.startIndex..<spaceIdx]
            let path = trimmed[trimmed.index(after: spaceIdx)...].trimmingCharacters(in: .whitespaces)
            guard let pid = pid_t(pidString) else { continue }
            if descriptor.bundlePathFragments.contains(where: { path.contains($0) }) {
                results.append((pid, path))
            }
        }
        return results
    }

    static func isRunning(_ app: BlueEarMeetingApp) -> Bool {
        !discoverProcesses(for: app).isEmpty
    }

    static func isInstalled(_ app: BlueEarMeetingApp) -> Bool {
        guard let descriptor = MeetingAppCatalog.descriptor(for: app) else { return false }
        return descriptor.installPaths.contains { FileManager.default.fileExists(atPath: $0) }
    }
}
