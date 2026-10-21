# Windows bulb controller

For a release download, extract the ZIP and run SmartControl.exe. First launch opens account setup. Left-click the notification-area bulb to open the graphical controls. Right-click for the menu, account setup and Quit. The local GNU development build is named bulb-tray.exe and also needs its libunwind.dll; release builds use MSVC and do not require that DLL.

## Controls

Click the large bulb card to toggle power. A cloud state read less than 15 seconds old is used immediately; otherwise the app reads state before toggling. This means changes from another controller can briefly make the displayed state stale. Click refresh to read the current state.

Drag the brightness slider and release to send one request. The swatches select warm white, white, red, green and blue. More opens the native colour picker. Colour choices retain brightness and turn the light on first if needed. Black sends off. Brightness retains colour and does not explicitly send on.

Click the bulb name, or right-click the tray icon and choose Rename bulb. The editor matches the panel theme. Edit the name and click Save name or press Enter; Cancel or Escape discards edits. Ctrl+Backspace deletes the previous word, Ctrl+A selects all, and normal undo works. The name is saved in client.json for this app; it does not rename the device in your cloud account.

Escape, the close button or clicking elsewhere hides the panel. Requests run on a worker thread. Rapid colour choices keep the latest choice queued; brightness and power requests are retained separately. The app checks reported state after commands, with bounded delayed reads and one later refresh if confirmation is slow. Cloud acceptance is not physical confirmation. Rate-limit responses show a cooldown message.

## First-time connection

Launch the app and click Connect account. Your default browser opens the account provider's official authorization page. Sign in and review access, then return to SmartControl, select your bulb and click Save connection. Setup only reads the device list; it does not change the bulb.

Right-click the tray icon and choose Connect / change account to authorize again or select another bulb. Closing setup cancels the attempt. Browser authorization expires after ten minutes, and errors can be retried.

The desktop integration currently uses the provider's public PKCE registration also used by its official CLI. The consent screen may identify that registration as SmartThings CLI. No CLI or Node process is run, and no client secret is embedded. This registration is provider-controlled; an independent production OAuth registration is a separate deployment consideration.

## Native Rust connection

All normal requests, browser callback handling and automatic renewal run in Rust. The app has no Node.js, Python or cloud CLI runtime dependency. It verifies a random OAuth state value and uses PKCE before exchanging an authorization code.

New connections store encrypted credentials in %LOCALAPPDATA%/SmartControl/auth.bin using Windows DPAPI for the signed-in Windows user. The app stores non-secret device settings in %LOCALAPPDATA%/SmartControl/client.json on a fresh installation. Existing portable client.json files remain supported; reconnecting updates that file. Keep authentication files private; they are not included in Git or release archives. Legacy plaintext sign-in files remain readable; renewed tokens are saved encrypted.

If you move the app to another Windows account or computer, connect the account again. Automatic renewal avoids repeated browser sign-in while the provider's authorization remains valid. Revoked or expired renewal access requires reconnecting from the tray menu.

## Linux server later

Quit the app, change client.json to the structure in client.server.example.json with your HTTPS server URL and the Windows client key, then restart. The server backend also uses Rust. No rebuild is required.

## Start at sign-in

Press Win+R, enter shell:startup, and put a shortcut to SmartControl.exe in that folder. No startup entry is installed automatically.

## Measurement

One read-only status request measured 0.37 seconds and approximately 9.4 MiB peak process working set with direct Rust access, versus 2.55 seconds and 104.9 MiB for the previous CLI request. These are single observations on this PC, not fixed limits. The physical bulb was unchanged by these checks.
