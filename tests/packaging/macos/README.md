# macOS Packaging Tests

Busytok bundle, LaunchAgent, and packaged smoke tests for the macOS 14+
SMAppService lifecycle.

## Test Files

- `cli_smoke.sh` — CLI help text test for the bundled `busytok` binary.
- `launch_agent_smoke.sh` — Verifies the SMAppService model: the service
  LaunchAgent plist is bundled into `Busytok.app/Contents/Library/LaunchAgents/`,
  no copy exists in `~/Library/LaunchAgents/`, and there is no
  `com.busytok.desktop-host.plist` (desktop-host uses `SMAppService.mainApp`).
- `installed_app_smoke.sh` — Verifies `Busytok.app` bundle layout, bundled
  service plist, absence of a bundled desktop-host plist, and the CLI
  "Open Busytok.app to start the background service" recovery message. Set
  `BUSYTOK_RUN_QUIT_SMOKE=1` to additionally exercise whole-product quit
  same-session suppression.
- `release_script_smoke.sh` — Verifies packaging scripts and the plist
  template exist.

## Environment Variables

| Variable                | Default                                          | Description                                                            |
|-------------------------|--------------------------------------------------|------------------------------------------------------------------------|
| `BUSYTOK_APP_PATH`      | `target/release/bundle/macos/Busytok.app`        | Path to the built `.app` bundle                                        |
| `BUSYTOK_RUN_QUIT_SMOKE`| `0`                                              | When set to `1`, also run the same-session quit suppression smoke      |

## Running

```bash
# CLI smoke (requires busytok built)
./tests/packaging/macos/cli_smoke.sh

# LaunchAgent smoke (requires .app built; does not require app to have run)
BUSYTOK_APP_PATH=target/release/bundle/macos/Busytok.app \
  ./tests/packaging/macos/launch_agent_smoke.sh

# Installed app smoke (requires .app built)
BUSYTOK_APP_PATH=/path/to/Busytok.app ./tests/packaging/macos/installed_app_smoke.sh

# Same-session quit suppression smoke (requires a logged-in GUI session)
BUSYTOK_APP_PATH=/path/to/Busytok.app BUSYTOK_RUN_QUIT_SMOKE=1 \
  ./tests/packaging/macos/installed_app_smoke.sh
```

## Expected State Under SMAppService

- The service LaunchAgent plist lives only at
  `Busytok.app/Contents/Library/LaunchAgents/com.busytok.service.plist`.
- The app registers the agent through `SMAppService.agent(plistName:)` at
  first launch and never writes a copy to `~/Library/LaunchAgents/`.
- `desktop-host` login-start is registered through `SMAppService.mainApp`;
  no `com.busytok.desktop-host.plist` is bundled or written to user home.
- `Quit Busytok Desktop` is a whole-product quit: it stops the GUI,
  desktop-host, and `busytok-service` for the current session without
  unregistering them, and they will not auto-respawn until the next explicit
  user action (e.g. reopening `Busytok.app` or a new login session).
