# Uninstalling Busytok

1. Quit Busytok Desktop:
   - Open the Busytok menu bar item and choose **Quit Busytok Desktop**, or
     quit the app from the Dock / `Cmd-Q`.
   - This stops the GUI, desktop-host, and `busytok-service` for the current
     session. `SMAppService` registrations are left in place until the bundle
     itself is removed in the next step, so they do not respawn.

2. Delete the application:
   - Move `Busytok.app` to Trash.
   - Removing the bundle also removes the bundled service LaunchAgent plist
     at `Contents/Library/LaunchAgents/com.busytok.service.plist`, so
     `SMAppService` no longer has a bundle to register from on next login.

3. Optional: remove the CLI shim (if installed):
   ```
   busytok cli uninstall
   ```
   Or delete `~/.local/bin/busytok` manually if the app bundle is already gone.

4. Optional: clear data and logs:
   - Delete `~/Library/Application Support/Busytok/`
   - Delete `~/Library/Logs/Busytok/`

5. Optional: revoke the login item (if you re-install later and want a clean
   approval prompt):
   - Open **System Settings → General → Login Items & Extensions**, find
     Busytok under "Allow in the Background", and toggle it off.
