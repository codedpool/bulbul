# Windows SmartScreen FAQ

When you first run Bulbul's installer on Windows, you'll likely see a blue dialog titled **"Windows protected your PC"** with an "OK / Don't run" button and no obvious way to continue.

This is normal for new open-source apps. Here's what's happening and how to install Bulbul anyway.

## What's the warning?

Windows SmartScreen is a reputation-based filter. Microsoft maintains a list of executables it has seen widely distributed without causing harm. Anything not yet on that list — especially software signed by a publisher Microsoft hasn't certified — gets flagged.

Bulbul is currently in this bucket because:

- It's a new project with few downloads
- The installer is **not** signed with a [Microsoft-trusted code-signing certificate](https://learn.microsoft.com/en-us/windows/security/application-security/application-control/windows-defender-application-control/select-types-of-rules-to-create) (those cost ~$100/year per cert; for a free side project this would mean charging users or eating the cost)

This is **not** the same as the installer being unsigned in general. Bulbul's installers **are** signed — with our own [minisign](https://jedisct1.github.io/minisign/) key, which the running app uses to verify auto-updates. SmartScreen just doesn't know about that key.

## How to install Bulbul anyway

1. When the **"Windows protected your PC"** dialog appears, click **More info** (small text near the top).
2. A new line will appear showing the app name (`Bulbul_x.y.z_x64-setup.exe`) and publisher (`Unknown publisher`).
3. Click the **Run anyway** button that now appears at the bottom.
4. The installer launches normally.

You'll only see this warning the first time. Once Bulbul is installed, neither the app itself nor the auto-update path triggers SmartScreen again.

## Should I trust this?

That's your call, and you should never accept "trust me, it's fine" from anyone shipping unsigned software. Three things you can do to verify before installing:

1. **Read the source.** Bulbul is open source under [GPL-3.0](../LICENSE). The entire codebase is at [github.com/codedpool/bulbul](https://github.com/codedpool/bulbul). If something looks off, don't install.
2. **Verify the installer signature.** Every release has a `.sig` file alongside the `.exe`. You can verify with [minisign](https://jedisct1.github.io/minisign/) using the public key embedded in `src-tauri/tauri.conf.json`:

   ```bash
   minisign -Vm Bulbul_1.0.0_x64-setup.exe -P <pubkey from tauri.conf.json>
   ```

   A "Signature and comment signature verified" response means the installer is byte-for-byte what was built from the tagged commit in this repo. This is exactly what Bulbul's own auto-updater checks before applying any update.
3. **Scan it.** Upload the `.exe` to [VirusTotal](https://www.virustotal.com/) and see what 70+ AV engines say.

## Why doesn't Bulbul just buy a code-signing certificate?

Three reasons:

- **Cost.** ~$100/year, recurring, for a free app the maintainer isn't monetizing.
- **EV certs** (the kind that bypass SmartScreen instantly) are even more expensive (~$300/year) and require hardware tokens.
- **Cert reputation builds slowly anyway.** Even a brand-new EV cert can trigger SmartScreen until enough users have installed software signed with it.

If Bulbul grows to the point where enough people are downloading it that the SmartScreen warning becomes a real obstacle, a code-signing cert is the obvious next step. For now, the **More info → Run anyway** path is the same one users follow for most small open-source Windows apps.

## After install

- Bulbul never re-triggers SmartScreen because Windows trusts running apps after their first successful launch.
- The auto-updater downloads new installers in the background but verifies them against the minisign public key embedded in the app. An attacker who somehow compromised your network or the GitHub release would still need the maintainer's private key + passphrase to ship a verified update. They don't have it.

If you ever hit a SmartScreen warning when updating, that's worth flagging in an issue — it shouldn't happen for an app already on your system.
