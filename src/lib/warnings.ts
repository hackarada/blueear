export function warningLabelFor(code: string): string {
  switch (code) {
    case "source_silent":
      return "No audio detected from the meeting app. Check that the call is unmuted.";
    case "source_restored":
      return "Meeting audio is flowing again.";
    case "source_process_tree_changed":
      return "Reconnecting to meeting audio...";
    case "source_app_not_found":
      return "The meeting app appears to have closed.";
    case "microphone_device_changed":
      return "Microphone device changed; restarting microphone capture.";
    case "native_error":
      return "A native audio error occurred.";
    case "disk_space_low":
      return "Disk space is low. Stopping recording to protect your files.";
    default:
      return code;
  }
}
