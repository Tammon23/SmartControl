# Publishing a Windows release

The Windows build and release workflow tests and builds a portable Windows x64 executable using MSVC with a static C runtime. The ZIP contains SmartControl.exe, instructions, a server configuration example and third-party notices. It contains no personal configuration or credentials. Extract it and run SmartControl.exe; first-time account setup opens automatically. This is a portable download, not an installer.

Pushes to main and manual runs create downloadable build artifacts under Actions. To publish a release:

1. Update workspace.package.version in Cargo.toml and regenerate Cargo.lock if needed.
2. Commit and push the change.
3. Create and push a matching tag, for example:

```sh
git tag v0.1.0
git push origin v0.1.0
```

After the workflow succeeds, download the ZIP from the repository's Releases page. The matching .sha256 file records its SHA-256 checksum. Future versions use new tags such as v0.1.1; do not move a published tag to different source.

The desktop OAuth integration currently relies on the provider's public PKCE registration also used by its CLI. See windows/START-HERE.md for details. No client secret or account token belongs in this workflow or its release assets.
