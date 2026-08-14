# Task 86 — Deployment image and supervised topology

**Plan:** [plan.md](../plan.md) · **Certificate:** [86-deployment_image_and_supervision-certificate.md](86-deployment_image_and_supervision-certificate.md)

**Implements:** [07-channels-and-operations.md §Deployment image](../../../07-channels-and-operations.md#deployment-image) · [07-channels-and-operations.md §Compatibility with letta-app-server-deployment](../../../07-channels-and-operations.md#compatibility-with-letta-app-server-deployment)
**Depends on:** 85
**Produces:** a non-root container image with the documented mounts, default command, and supervised channel host, alongside the deployment-variant comparison
**Pointers:** `Dockerfile`, `compose.yaml`, `docs/deployment.md`; reference: `../letta-app-server-deployment/`, `../letta-code/src/cli/subcommands/listen.tsx`

## Steps

- [ ] Build an image containing the `lotta` binary, Git, CA certificates, and the optional OS sandbox runtime
- [ ] Include Bun or Node only when JS compatibility hosts or channels are enabled, and OS cron only when explicitly configured
- [ ] Set the default command to `lotta server --backend local --listen ws://0.0.0.0:4500 --ws-auth capability-token --ws-token-file /run/secrets/lotta-app-server-token`
- [ ] Mount `/home/lotta/.letta` as the state volume with `LETTA_HOME` pointing at it, `/home/lotta/.letta/lc-local-backend` as `LETTA_LOCAL_BACKEND_DIR`, `/run/secrets/*` read-only, and `/workspace` optional
- [ ] Run as a non-root user with a read-only root filesystem, dropped Linux capabilities, and write access only to state, workspace, and temp paths
- [ ] Supervise the channel host as a child in the same service and document the deployment-variant differences from `letta-app-server-deployment`

## Definition of done

- [ ] The image runs as a non-root user with a read-only root filesystem, dropped capabilities, and write access only to state, workspace, and temp
- [ ] The default command matches the §Deployment image block, and the four mount points resolve with `LETTA_HOME` and `LETTA_LOCAL_BACKEND_DIR` set correctly
- [ ] Bun or Node is present only when JS compatibility hosts or channels are enabled, and OS cron only when explicitly configured
- [ ] The channel host is supervised as a child in the same service, and the deployment-variant comparison is documented
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `docker build .` then starts the image with a token file mounted and sees the server bind on 4500 as a non-root user, `/readyz` return ready, and the channel host running as a supervised child
