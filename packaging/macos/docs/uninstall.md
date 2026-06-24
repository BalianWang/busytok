# Uninstalling Busytok

1. Quit Busytok Desktop:
   - Open the Busytok menu bar item and choose **Quit Busytok Desktop**.
     This is the guaranteed whole-product quit path — it stops the GUI,
     the `busytok-service`, and the login-start helper for the current session.
   - Dock "Quit" can bypass the full shutdown pipeline and may leave the
     `busytok-service` running.  `Cmd-Q` normally routes through the same
     pipeline as the menu bar quit.  To ensure a clean shutdown, use the
     menu bar **Quit Busytok Desktop** option.

2. Delete the application:
   - Move `Busytok.app` to Trash.

3. Remove the background-service launch agent:
   - The service is registered from a managed plist the app writes to
     `~/Library/LaunchAgents/com.busytok.service.plist`. Deleting the bundle
     alone leaves this plist registered (it still points at the now-deleted
     binary). To fully stop and unregister the service:
   ```
   launchctl bootout gui/$(id -u)/com.busytok.service
   rm -f ~/Library/LaunchAgents/com.busytok.service.plist
   ```
   (The `bootout` may report "not loaded" if the service already exited —
   that is fine.)

4. Optional: remove the CLI shim (if installed):
   ```
   busytok cli uninstall
   ```
   Or delete `~/.local/bin/busytok` manually if the app bundle is already gone.

5. Optional: clear data and logs:
   - Delete `~/Library/Application Support/Busytok/`
   - Delete `~/Library/Logs/Busytok/`

6. Optional: revoke the login item (if you re-install later and want a clean
   approval prompt):
   - Open **System Settings → General → Login Items & Extensions**, find
     Busytok under "Allow in the Background", and toggle it off.
