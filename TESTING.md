# Validation record — 20 October 2026

## Windows app

- Native Windows x64 release executable built successfully with Rust 1.99.1 and the GNU/LLVM toolchain.
- Executable size: 1,370,112 bytes, plus the included libunwind runtime DLL.
- A read-only live check returned the configured bulb as on, brightness 50%, 5000 K, online.
- The process started successfully and its initial idle working set was approximately 8.1 MiB. This is one observation, not a memory guarantee; transient Node/CLI requests have separate memory use.
- The user confirmed the yellow tray icon and controls open.
- The same executable's HTTP backend successfully read a local demo server with its own client key.
- No startup entry was installed. The app remains running in the current user session.

## Server

Six integration tests passed:

1. Unauthorized writes cannot change state.
2. Malformed/out-of-range input cannot change state.
3. Independent client keys share the same state and consistent colour behaviour.
4. Token refresh is coalesced and the rotated refresh token is persisted.
5. Upstream requests have the expected command shape; upstream errors are not reported as success or leaked to clients.
6. Client keys cannot initiate administrative OAuth; invalid callback state is rejected.

A release build of the server ran on the local Windows test machine in demo mode. Live HTTP tests verified authenticated status and state changes, rejection of unauthorized reads, and interoperability with the tray client. The temporary demo server was stopped afterward. No real bulb requests were made by the new server.

The Dockerfile and Oracle installation procedure have not been executed on a Linux/Oracle host. The package includes source for a native Linux build, not a prebuilt Linux binary. Real SmartThings OAuth registration and server account linking remain deployment tasks.

## Packaging

ZIP archives are tested for archive integrity and carry a SHA-256 manifest of their own contents. No server configuration with generated keys, Samsung OAuth tokens, account passwords, or SSH credentials is included. The ready-to-run Windows config contains only local program paths and the bulb's non-secret SmartThings identifier.

## Graphical tray update

The graphical panel uses native Win32/GDI. A 360 by 460 preview was rendered using the same paint code and visually inspected. The release executable was rebuilt, its real SmartThings read-only connection check reported on at 50 percent and 5000 K, and the tray process was restarted successfully. No physical bulb commands were sent during this update's verification. Mouse interaction with the updated panel still needs user confirmation.
Two local HTTP mock tests passed: toggling reads fresh state and sends the opposite power value for both states; an unknown power state prevents a write.

## Direct Rust connection update

A read-only native status check succeeded in 0.37 seconds with approximately 9.4 MiB sampled peak working set. Forced token expiry successfully exercised native renewal and another status read. These checks left the bulb on at 50 percent, 5000 K. No write command was sent to the physical bulb during this update. Native direct writes batch the existing command payloads into one request; their physical execution was not tested in this environment. The original Node backend was removed.
The installed native tray process measured 11.1 MiB working set and 0.06 CPU-seconds after startup and its initial status read. Three release tests passed, including toggle safety and rename persistence. The refreshed graphical preview was visually inspected; its footer reads Connected.
Name editor update: themed input, Save and Cancel controls, Enter/Escape shortcuts and Ctrl+Backspace. Five release tests passed, including a hidden native Windows edit-control test that verifies word deletion, selected-text deletion and undo, plus Unicode/punctuation boundary cases. The rename preview was rendered and visually inspected. No physical bulb write commands were used for verification.

## Native sign-in setup

Nine Windows release tests passed. New tests verify the RFC 7636 PKCE challenge, mandatory OAuth state and duplicate-parameter rejection, compatible-device filtering, and a Windows DPAPI credential round trip with tamper rejection. Existing edit-control, rename, and toggle tests still pass. A live read-only check verified compatibility with the pre-existing connection before opening the browser setup test. Setup reads device information and does not issue bulb commands. The user completed end-to-end browser authorization, selected the bulb and saved the connection; the tray icon returned. A subsequent read-only check through the encrypted store succeeded and reported on at 50 percent.

## Colour responsiveness update

Thirteen native tray tests and eight server tests passed. New coverage checks single colour command payloads, accepted-result validation, queued latest colour selection without losing other controls, stale readback rejection, colour/white mode inference and server request sequencing. The user confirmed controlled green-to-blue changes and then rapid tray green/blue/red selections: the bulb and icon matched and controls stayed responsive. Sampled cloud requests returned HTTP 200, with no observed rate-limit response in these tests. The native client now reports HTTP 429 cooldowns, bounds confirmation polling and schedules one later refresh when needed. Oracle deployment remains unperformed.

## Daily automation update

The shared scheduler passed five tests covering validation, sunset scheduling, daylight-saving transitions and once-per-day execution. Fifteen tray tests and eleven server tests passed in the release profile. New coverage includes action ordering and durable server claims that prevent replay after failure or restart. New rules are disabled by default. The native automation editor preview was visually inspected; interactive editor testing remains pending. CI now runs all three crates' tests. No v0.1.1 release has been published.

## Resizing and custom automation colours

Follow-up: the tray panel was restored to its original fixed size at the user's
request. Only Automations is resizable. Opening the tray controls explicitly
destroys any remaining automations window. The native close test covers both
WM_CLOSE and repeated programmatic closure, and the user confirmed the desired
window behaviour. The following font/reconnect update passed all 34 Rust tests;
rendered editor and sign-in-error previews were visually inspected, launch checks
passed, and the local app was replaced and restarted. Automation text and fields
are larger. Sign-in errors expose Reconnect; other errors expose full Details.

Seventeen tray tests, six scheduler tests and eleven server tests passed. A native hidden-window test resizes the rule editor and verifies that edit contents and selection survive. The same native check was rerun with WM_CLOSE and verified destruction plus clearing the cached window handle. Custom RGB actions round-trip through settings, reject conflicting presets/black/off combinations, run before final brightness, and execute through the server's demo scheduler. Both native windows now resize, the tray panel opens Automations directly, and rule actions expose a native colour picker. Rendered previews were visually inspected, launch checks passed, and the local tray app was replaced and restarted. The Oracle runtime has not been updated for the custom RGB schedule field. Physical rule execution and live UI interaction still need user confirmation.

## Start at sign-in setting

The tray Settings submenu now has a checked Start at sign-in toggle. It registers
or removes only SmartControl's named per-user Run value, preserving the executable
and configuration paths. Nineteen tray tests, six scheduler tests and eleven
server tests passed. New tests use an isolated temporary registry key to verify
enable/disable, repeated removal and preservation of unrelated values, plus
Windows command-line parsing with spaces and Unicode. The user's actual startup
entry was absent and was not enabled during testing. A real sign-out/sign-in
cycle has not been performed.

## v0.1.1 size and runtime review

The Windows-only scheduler now uses the operating system's local timezone without
bundling chrono-tz's world timezone database. The server retains its IANA timezone
support. All 37 workspace tests passed, plus the isolated Windows/local-timezone
configuration (19 tray tests and one scheduler test). No cloud polling intervals
or command pacing were shortened.

On this PC, the GNU/LLVM executable decreased from 2,926,592 to 1,664,512 bytes
(43.1%). The previous running app sampled at 14.68 MiB working set; the freshly
started optimized app sampled at 11.31 MiB, with 2.56 MiB private memory. These are
observations with different process histories, not a controlled RAM benchmark.
The optimized process accumulated 0 ms CPU over a 12.02-second idle sample and
93.75 ms CPU through its first three seconds after startup. A separate read-only
status request took 0.466 seconds wall time, 62.5 ms CPU and approximately 9.68 MiB
sampled peak working set; it did not change the bulb. Public releases use MSVC,
so their exact file size and memory usage can differ from these local GNU builds.
