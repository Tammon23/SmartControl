# Windows bulb controller

Keep bulb-tray.exe, libunwind.dll and client.json together. Left-click the notification-area bulb to open the graphical controls. Right-click for the menu and Quit.

## Controls

Click the large bulb card to toggle power. A cloud state read less than 15 seconds old is used immediately; otherwise the app reads state before toggling. This means changes from another controller can briefly make the displayed state stale. Click refresh to read the current state.

Drag the brightness slider and release to send one request. The swatches select warm white, white, red, green and blue. More opens the native colour picker. Colour choices also turn the light on. RGB choices derive brightness from the selected colour value; black sends off. Brightness retains colour and does not explicitly send on.

Click the bulb name, or right-click the tray icon and choose Rename bulb. The editor matches the panel theme. Edit the name and click Save name or press Enter; Cancel or Escape discards edits. Ctrl+Backspace deletes the previous word, Ctrl+A selects all, and normal undo works. The name is saved in client.json for this app; it does not rename the device in your cloud account.

Escape, the close button or clicking elsewhere hides the panel. Requests run on a worker thread. The app reads back reported state after commands; cloud acceptance is not physical confirmation. There is no continuous background polling.

## Native Rust connection

The current backend is direct_cloud. All normal requests and automatic sign-in renewal run in Rust. The application does not start Node.js, Python or a cloud CLI. Colour commands are batched into one HTTP request. The original sign-in was migrated from the prior setup.

client.json contains the device ID and a path to the sign-in file under your Windows local app data, normally %LOCALAPPDATA%/BulbBridge/auth.json. That file contains account credentials: keep it private. It is excluded from the download archives. The ZIP is therefore not a signed-in installation for another computer. If sign-in is revoked or its refresh token expires, the account must be authorized again. A first-time sign-in screen is not included in the tray app yet.

## Linux server later

Quit the app, change client.json to the structure in client.server.example.json with your HTTPS server URL and the Windows client key, then restart. The server backend also uses Rust. No rebuild is required.

## Start at sign-in

Press Win+R, enter shell:startup, and put a shortcut to bulb-tray.exe in that folder. No startup entry is installed automatically.

## Measurement

One read-only status request measured 0.37 seconds and approximately 9.4 MiB peak process working set with direct Rust access, versus 2.55 seconds and 104.9 MiB for the previous CLI request. These are single observations on this PC, not fixed limits. The physical bulb was unchanged by these checks.
