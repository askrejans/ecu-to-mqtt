# Future project rename — plan only

**No rename has been performed.** Version 0.4.0 retains `speeduino-to-mqtt` as
the repository, Cargo package, executable, service and installation identity.
This document is a guide for a separately authorized rename. It does not authorize
a repository move, release, upload or push.

A possible future name is **`ecu-to-mqtt`**: it describes the multi-ECU purpose
and matches the neighboring `gps-to-mqtt`, `sensors-to-mqtt` and `can-to-mqtt`
projects. Confirm availability and the final name before executing this plan.
The examples below use that proposal, not an adopted name.

## 1. Preserve compatibility first

- Keep accepting `SPEEDUINO_*` environment variables for at least one announced
  migration release. If adding `ECU_*`, define new variables as taking precedence
  and test the conflict rules. Never silently reinterpret a saved ECU profile.
- Preserve configured MQTT prefixes and the default `/speeduino/ecu/` topic during
  the rename. Broker ACLs, dashboards, retained data and phone settings depend on
  those paths. Offer an explicit topic migration separately, with a unique car
  prefix and coordinated subscriber update.
- Preserve existing TOML keys, protocol names and canonical JSON schema.
- Provide an old executable alias or wrapper during the migration. Ensure package
  installation owns that alias so upgrades and uninstalls remain predictable.
- Preserve current config files and service account ownership. Choose either a
  compatibility config path or a one-time backup-and-copy migration that never
  overwrites an existing destination. Do not put passwords into migration logs.
- Stop the old service before enabling the new service. Two instances must not
  contend for the same serial port or publish duplicate data under the same topic.

## 2. Repository-specific changes

| Location | Change when the rename is approved |
|---|---|
| Git hosting repository and local checkout directory | Rename to the chosen name, confirm old URL redirects, then update local remotes and downstream clones. Do not delete/recreate the repository. |
| `Cargo.toml`, `Cargo.lock` | Change package identity, deliberately choose binary identity, regenerate the lockfile with Cargo and verify `cargo metadata`. |
| `src/main.rs`, `src/mqtt_handler.rs`, `src/config.rs` | Update CLI/banner/client-ID branding; add tested environment aliases if desired. Keep `ecu_protocol = "speeduino"` and manufacturer names intact. |
| `Dockerfile`, `docker-compose.yml`, `.dockerignore` | Update binary and config copy paths, command, service/image names and labels. Keep a migration image tag if images were previously published. Build locally before any registry work. |
| `speeduino-to-mqtt.service` | Rename the unit file, update `ExecStart`, `Documentation`, log identifier and chosen config path. Preserve or migrate the existing `speeduino` system account and device groups explicitly. |
| `scripts/build_packages.sh` | Update `PKG_NAME`, descriptions, URL, service source, DEB/RPM install/uninstall scripts and Windows README/NSSM instructions. Some names are hard-coded beyond `PKG_NAME`; inspect all matches. |
| `example.settings.toml`, `examples/`, `README.md`, `ECU_PROTOCOLS.md` | Update commands and branding while explaining old names and settings that remain supported. |
| `../../homebrew/homebrew-g86racing/Formula/speeduino-to-mqtt.rb` (sibling repository) | Rename formula/class, update URLs, checksums, binary and launchd paths. Use Homebrew's formula rename mechanism for upgrades; do not overwrite users' configs. |
| Package hosting and any release automation | Audit externally: APT/RPM names and metadata, macOS/Windows artifact filenames, checksums, signatures, download pages, CI secrets and container tags. Publishing is a separate authorized action. |

The sibling formula is actually located at
`../../homebrew/homebrew-g86racing/Formula/speeduino-to-mqtt.rb` relative to this
repository. The project group `AGENTS.md`, G86 app documentation and other
consumer references also need an inventory before the eventual rename.

Use a targeted search rather than a global replacement:

```sh
rg -n --hidden -g '!.git/**' -g '!target/**' -g '!release/**' \
  'speeduino-to-mqtt|SPEEDUINO_|SpeeduinoToMqtt|/etc/speeduino|User=speeduino|Group=speeduino'
```

`SpeeduinoData`, Speeduino firmware protocol names and compatibility topic keys
are legitimate manufacturer-specific identifiers and should retain their names.

## 3. Validate locally before release

1. Build and run unit/integration tests under the new package name. Confirm
   `--help`, default configuration, explicit `--config`, `.env`, legacy variables
   and any new variable precedence work.
2. Replay Speeduino and every supported ECU fixture. Compare MQTT topic, payload,
   units and freshness behavior with the pre-rename build; a rename must not
   change recorded data or broker permissions.
3. Test clean install and upgrade from 0.4.0 for DEB, RPM, Homebrew/launchd and
   Windows/NSSM. Confirm exactly one service runs and serial access survives.
4. Build the Docker image locally; test both mounted old configs and new example
   configs. Verify that no secret or generated build directory enters the image.
5. Confirm old command/config compatibility, then test uninstall and rollback.
   Retain the prior packages, configuration backup and previous Git tag so the
   old service can be restored without changing topics or ECU settings.
6. Review the migration notes and release artifacts. Only after separate release
   authorization update hosting, repositories, taps, registries or package feeds.

## Current status

- [x] Document rename scope and compatibility requirements.
- [ ] Approve final project name and migration policy.
- [ ] Implement and verify rename locally.
- [ ] Authorize and perform any external repository rename or publication.
