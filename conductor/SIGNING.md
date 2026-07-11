# Code Signing — why and how

macOS keys every permission grant (Full Disk Access, Desktop/Documents/Downloads
access, Photos, microphone…) to the app's **code signature**. Ad-hoc-signed builds
(the default without a certificate) get a *different* signature on every build, so
macOS forgets all grants each time the app updates — that's the root cause of the
repeated permission dialogs in
[issue #3](https://github.com/arunkarnann/aran-terminal/issues/3).

A **stable** signing identity fixes this. No paid Apple Developer account is
needed for grants to persist — a free self-signed certificate works. (A paid
Developer ID certificate additionally satisfies Gatekeeper; see the last section.)

## 1. Create a self-signed signing certificate (one-time)

1. Open **Keychain Access** (Applications → Utilities).
2. Menu bar: **Keychain Access → Certificate Assistant → Create a Certificate…**
3. Fill in:
   - **Name:** `Aran Terminal Signing`
   - **Identity Type:** Self-Signed Root
   - **Certificate Type:** **Code Signing**
4. Click **Create**, then **Done**.
5. First use of the cert pops a keychain confirmation — click **Always Allow**.

## 2. Sign local builds

```sh
APPLE_SIGNING_IDENTITY="Aran Terminal Signing" npm run tauri build
```

Verify the app is no longer ad-hoc signed:

```sh
codesign -dv --verbose=2 "src-tauri/target/release/bundle/macos/Aran Terminal.app" 2>&1 | grep -E "Authority|Signature"
# Should show Authority=Aran Terminal Signing — NOT "Signature=adhoc"
```

> Don't hardcode `signingIdentity` in `tauri.conf.json` — contributors without
> the cert would fail to build. The env var keeps ad-hoc as the default.

**Heads-up:** the first build after switching from ad-hoc to the certificate
resets existing permission grants **once** (the signature changed). After that,
grants persist across every rebuild and update.

## 3. Sign CI release builds (optional)

1. Export the certificate + private key from Keychain Access: right-click the
   cert → **Export…** → format `.p12`, choose a password.
2. Base64-encode it: `base64 -i AranTerminalSigning.p12 | pbcopy`
3. Add repo secrets (Settings → Secrets and variables → Actions):
   - `APPLE_CERTIFICATE` — the base64 string
   - `APPLE_CERTIFICATE_PASSWORD` — the `.p12` password
   - `APPLE_SIGNING_IDENTITY` — `Aran Terminal Signing`

`tauri-action` imports the cert into a temporary keychain automatically (already
wired in `.github/workflows/release.yml`). Set all three secrets together or none.

## 4. Appendix: scripted cert creation (no GUI)

```sh
# Generate key + self-signed code-signing cert, then import into the login keychain.
openssl req -x509 -newkey rsa:2048 -keyout key.pem -out cert.pem -days 3650 \
  -nodes -subj "/CN=Aran Terminal Signing" \
  -addext "keyUsage=digitalSignature" -addext "extendedKeyUsage=codeSigning"
openssl pkcs12 -export -inkey key.pem -in cert.pem \
  -out AranTerminalSigning.p12 -passout pass:changeme
security import AranTerminalSigning.p12 -k ~/Library/Keychains/login.keychain-db \
  -P changeme -T /usr/bin/codesign
rm key.pem cert.pem
```

If `codesign` later reports the identity as untrusted, open the cert in Keychain
Access → Trust → set **Code Signing** to **Always Trust**.

## 5. Limitations, and upgrading to Developer ID later

- Self-signing does **not** satisfy Gatekeeper: downloaded builds still need
  right-click → **Open** (or `xattr -d com.apple.quarantine`) on first launch.
- With a paid Apple Developer account later: export the **Developer ID
  Application** cert as `.p12` and just replace the three secret values, plus add
  `APPLE_ID`, `APPLE_PASSWORD` (app-specific password) and `APPLE_TEAM_ID`
  secrets to enable notarization in `tauri-action`. Nothing else changes.
