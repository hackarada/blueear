# Security Policy

## Supported versions

Security fixes are accepted against the latest `main` branch. There are no
long-term support releases yet.

## Reporting a vulnerability

Please report security issues privately. Prefer GitHub Security Advisories on
the [blueear repository](https://github.com/hackarada/blueear/security/advisories/new).
If that is unavailable, contact the maintainer via the email on their GitHub
profile.

Do not open a public issue for vulnerabilities that could affect users' audio
privacy, local data integrity, or privilege escalation.

Include:

- A clear description of the issue and impact
- Steps to reproduce, or a minimal proof of concept
- Affected platform (macOS / Windows) and app version or commit when known

You should receive an acknowledgment within a reasonable time. Please give the
maintainer time to investigate and fix before any public disclosure.

## Design notes

Blue Ear is local-first by design:

- Recordings and transcripts stay on the user's machine under the Music /
  app-support folders described in the README.
- There is no intentional network upload path for meeting audio in the current
  product scope.
- Optional transcription providers run locally (Apple Speech, FluidAudio, or
  Whisper.cpp). Model bundles are imported by the user and verified against
  manifest digests before use.

Reports that involve unintended network access, path traversal outside the
intended recording/app-support roots, or unsafe handling of imported model
bundles are especially welcome.
