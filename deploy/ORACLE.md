# Prepare for Oracle Linux deployment

The package is ready for a later upload. No SSH credentials are needed until deployment. Choose an Oracle AMD64 or ARM64 Linux VM, preferably with at least 2 GB RAM for a native Rust build; a small VM may need swap or a build performed elsewhere. Runtime use is much smaller than compiling. Build memory and performance depend on the host.

The commands below assume Docker with the Compose plugin and a shell on your VM. They have not been executed against your Oracle account. Review host-specific installation instructions before installing Docker. Do not upload `windows/client.json` as a server configuration: it contains Windows-only CLI paths.

## 1. Upload and build

Upload the source deployment archive and extract it into a directory you own. In the directory containing `Dockerfile` and `compose.yaml`:

```sh
docker compose build
```

This builds natively for the VM architecture. The image uses an unprivileged user and has no Node.js, Python or SmartThings CLI dependency.

## 2. Generate private configuration

Generate configuration using the image built in step 1. Use the image directly so Compose does not try to bind-mount a configuration file that does not exist yet:

```sh
umask 077
docker create --name bulb-bridge-init bulb-bridge:local init /tmp/config.json
docker start -a bulb-bridge-init
docker cp bulb-bridge-init:/tmp/config.json ./config.json
docker rm bulb-bridge-init
chmod 600 config.json
sudo chown 10001:10001 config.json
```

Before leaving demo mode, replace the placeholder `device_id` in `config.json` with your own device UUID.

`init` generates different random keys for Windows, Kindle and administration, sets a placeholder device UUID, and enables **demo mode**. It never overwrites a file. Keep `config.json` private. For Docker, the image overrides the bind address to `0.0.0.0:8787`; Compose exposes that port only on the host's loopback interface.

Start in demo mode:

```sh
docker compose up -d
curl --fail http://127.0.0.1:8787/health
```

Use the Windows client key to test `/api/bulb` as documented in [API.md](../API.md). Demo commands affect only memory and reset after restart; no physical bulb changes.

## 3. Add HTTPS

Point a domain such as `bulb.example.com` at the VM. Configure Caddy on the host using `Caddyfile.example`, with your domain. Allow TCP 80 and 443 in both Oracle network rules and the Linux firewall. Keep port 8787 closed to the public internet.

Caddy proxies to `127.0.0.1:8787` and manages certificates. Validate HTTPS from a browser before configuring the Kindle. Use a normal domain certificate with DNS Subject Alternative Names. The Kindle plugin requires a CA bundle and checks the hostname as well as the certificate chain; it does not offer a switch to bypass TLS verification.

## 4. Authorize your own SmartThings app

The working Windows CLI connection stays on Windows. Its tokens are not exported or used as a permanent server credential.

Register your own SmartThings OAuth/API Access app with the exact redirect URI:

```text
https://bulb.example.com/oauth/callback
```

Request device read and execute permissions. Start with the [official SmartThings quick start](https://developer.smartthings.com/docs/getting-started/quickstart) and use the available app-creation flow for your account. Platform availability, terms, quotas, and any account plan requirements must be checked there; this package does not create or purchase an app registration.

Use `sudoedit config.json` to set the `oauth` field to the structure in `oauth.example.json`, with your own client ID, client secret and callback URL. Change `demo` to `false`. Keep `tokens_file` set to `data/tokens.json`. The persistent Docker volume stores tokens separately from the read-only config.

Preserve ownership for the image's unprivileged UID 10001 and restart:

```sh
sudo chown 10001:10001 config.json
docker compose up -d --force-recreate
```

Use the admin key from your private config:

```sh
read -r -s -p 'Admin key: ' BULB_ADMIN_KEY; echo
curl --fail-with-body -X POST \
  -H "Authorization: Bearer $BULB_ADMIN_KEY" \
  https://bulb.example.com/admin/oauth/start
unset BULB_ADMIN_KEY
```

Open the returned `authorize_url` on your own computer, sign in to Samsung, and authorize the location containing your bulb. Completion displays “Bulb Bridge is authorized.” The server exchanges and stores tokens, then refreshes them as required. Credentials never go to the Kindle or tray client. Back up the token volume securely if you need to preserve authorization.

If your account requires a different official OAuth endpoint, `oauth.authorize_url` and `oauth.token_url` can be explicitly configured; the defaults match the existing documented SmartThings OAuth flow. Real server OAuth has not yet been verified against your account. Upstream OAuth testing in this package uses a mock provider.

### Optional one-day REST test

A PAT can test the server before app registration, but it expires after 24 hours and cannot refresh itself. Create `data/tokens.json` **inside the persistent server data volume** with:

```json
{"access_token":"YOUR_TEST_PAT","refresh_token":null,"expires_at":0}
```

Use a real numeric expiry before the PAT's expiry, and set `demo` to false. Do not copy this placeholder literally. PAT creation and storage are manual; no PAT is included in this package. Use the OAuth route for unattended operation.

## 5. Connect clients

- Windows: quit the tray, configure `client.json` using `client.server.example.json`, then restart. Use the **Windows** key.
- Kindle: install the new plugin and enter the server URL, **Kindle** key and CA bundle path. See the README in the separate KindleControl package.

Test Read status before issuing any changes. Then try one action at a time and verify both cloud state and the physical bulb.

## Alternative native systemd deployment

Install Rust and OpenSSL development prerequisites, then build with `cargo build --locked --release -p bulb-server`. Create a dedicated `bulbbridge` system user and `/opt/bulb-bridge/data` owned by it. Install the binary at `/usr/local/bin/bulb-server`, private config at `/opt/bulb-bridge/config.json`, and adapt the included `bulb-bridge.service`. The service defaults to loopback; use the same HTTPS reverse proxy. This path also needs authorization and client configuration.
