# Deployment files

The files that install a Junto server with Docker Compose. The steps are in [`docs/install.md`](../docs/install.md); the decisions are in ADR 0003, 0004, 0014, 0015, 0016, 0018, 0021 and 0022.

| Path | Content |
|---|---|
| `env.example` | Every setting, explained. Copy it to `.env`, which is not tracked. |
| `generate-secrets.sh` | Prints the secret settings with new random values. |
| `check-limits.sh` | Names each CPU limit of a form that is above the CPUs of the host, before the first start. |
| `base.yaml` | Each service, defined once. Not used alone. |
| `one-host.yaml` | Form 1: every component on one host with a public address. |
| `two-hosts-data.yaml`, `two-hosts-public.yaml` | Form 2: the data host and the public host. |
| `local.yaml` | Every component on one machine without DNS names and TLS, for a trial and for the tests. |
| `worker-gpu.yaml` | Addition to a form: the transcription worker on an NVIDIA GPU. |
| `backup.sh` | Backup of the database, the attachments and the recordings while the server runs. |
| `restore.sh` | Restore of such a backup. An interrupted run is followed by a second run. |
| `restore-images.yaml` | Restore override: requires explicit prebuilt server and init images and removes their build definitions. |
| `upgrade-undo.sh` | The way back to the copy of the database from before a migration. |
| `database-user.sh` | Once, for a deployment of an earlier release: gives the database to the role of the server. |
| `entry/Caddyfile` | The entry on port 443 with three server names. |
| `images/server/` | The server image: Dockerfile, hand-over of transcription jobs, volume owners. |
| `images/worker/` | The loop that runs transcription jobs in the worker container. |
| `images/recorder/` | The start script of the recorder, which saves partial files. |
| `images/link/` | The image of the private link of form 2. |
| `images/postgres/` | The access rules of PostgreSQL, and the file that creates the role of the server. |
| `tests/` | Tests that need Docker and no public address. |

## Tests

```sh
tools/check-pins.sh                                   # pinned versions agree
deploy/tests/compose-config.sh                        # every form renders and follows the rules
python3 -B -m unittest discover -s deploy/tests -p 'test_restore_images.py' # restore image checks before container changes
docker build -f deploy/images/server/Dockerfile -t junto-server .
deploy/tests/transcribe-handover.sh junto-server      # the hand-over scripts, with two users
deploy/tests/recorder-start.sh junto-server           # the start script of the recorder
deploy/tests/entry.sh                                 # the entry selects by server name
deploy/tests/smoke.sh                                 # the local form starts, with the database role of the server
deploy/tests/backup-restore.sh                        # backup of a running server, refused restores, an interrupted restore
deploy/tests/upgrade.sh                               # upgrade from an earlier release, the database role, and the way back
deploy/tests/one-host.sh                              # the one-host form on one machine
deploy/tests/two-hosts.sh                             # the two-host form on one machine, as two projects
(cd spikes/test-participant && cargo build --release)
deploy/tests/recording.sh                             # a recorded call with the real recorder, and three failures
```

The last five build the images that do not exist yet, give their containers memory limits under 3 GB together, and remove everything at the end. `upgrade.sh` needs the commit of the earlier release in the checkout, and also builds the server image of that release. `one-host.sh` and `two-hosts.sh` need the ports 80, 443, 3478, 5349, 7880, 7881 and 7882 of the machine, because the media server and the entry use the network of the host; run one of them at a time.
