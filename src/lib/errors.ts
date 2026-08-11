import { isAppError } from "../types/recording";

// Rust already writes user-safe messages for every error code (see the doc
// comment in src-tauri/src/error.rs), so the message is passed through rather
// than remapped here. The fallback exists for the rare non-AppError rejection.
export function describeError(err: unknown, fallback = "Something went wrong. Please try again."): string {
  if (isAppError(err)) return err.message;
  if (typeof err === "string") return err;
  return fallback;
}
