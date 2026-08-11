// MeetingApp.swift
//
// Closed set of meeting apps Blue Ear can tap. The Int32 raw values are part
// of the C ABI with Rust (`MeetingAppId`); never renumber without updating
// `src-tauri/src/audio/meeting_app.rs` and its discriminant parity test.
//
// SECURITY-REVIEW: process taps are scoped exclusively to allowlisted bundle
// path fragments from this table. The UI never supplies arbitrary paths or
// PIDs.

import Foundation

@objc public enum BlueEarMeetingApp: Int32 {
    case teams = 0
    case zoom = 1
}

struct MeetingAppDescriptor {
    let id: BlueEarMeetingApp
    let displayName: String
    let wireId: String
    /// Substrings matched against `ps` executable paths.
    let bundlePathFragments: [String]
    /// Absolute app bundle paths checked for "installed" readiness.
    let installPaths: [String]
}

enum MeetingAppCatalog {
    static let all: [MeetingAppDescriptor] = [
        MeetingAppDescriptor(
            id: .teams,
            displayName: "Microsoft Teams",
            wireId: "teams",
            bundlePathFragments: ["Microsoft Teams.app", "Microsoft Teams classic.app"],
            installPaths: [
                "/Applications/Microsoft Teams.app",
                "/Applications/Microsoft Teams classic.app",
            ]
        ),
        MeetingAppDescriptor(
            id: .zoom,
            displayName: "Zoom",
            wireId: "zoom",
            bundlePathFragments: ["zoom.us.app"],
            installPaths: ["/Applications/zoom.us.app"]
        ),
    ]

    static func descriptor(for id: BlueEarMeetingApp) -> MeetingAppDescriptor? {
        all.first { $0.id == id }
    }

    static func fromRaw(_ raw: Int32) -> BlueEarMeetingApp? {
        BlueEarMeetingApp(rawValue: raw)
    }
}
