# Manual system test

Checks that need root, real hardware, or a person in front of the camera.
Run them after changes to the PAM module, daemon, D-Bus/polkit files, or
installation.

Keep a root shell open in a separate terminal (`sudo -i` before you start)
for the whole session, so a broken PAM stack can be reverted. Follow the
logs in another terminal:

```bash
sudo journalctl -f | grep -E 'pam_faceauth|faceauth-daemon|faceauth-agent|polkit'
```

## 1. sudo and polkit policy

PAM authenticates the *target* account. With openSUSE's default
`Defaults targetpw`, `sudo` asks for root's credentials, so your face cannot
approve it (this is the root-escalation fix working as intended); `sudo make
install` prints a note and `faceauth doctor` warns in its `sudo` check. To
use face auth for `sudo`, authenticate as yourself:

```bash
sudo zypper install sudo-policy-wheel-auth-self   # members of wheel use their own password
sudo usermod -aG wheel "$USER"                    # then log out and back in
```

Likewise polkit "admin" actions authenticate root unless a polkit rule makes
wheel members administrators (`polkit.addAdminRule` in
`/etc/polkit-1/rules.d/`); `auth_self` actions (faceauth's own `manage-own`)
always authenticate you.

## 2. Install

```bash
make && make test
sudo make install
```

Expect `faceauth-setup` to report "enabled pam_faceauth in /etc/pam.d/..."
for sudo, sudo-i, and polkit-1 (kde-fingerprint already has the module).
After the first install, log out and back in so XDG autostart starts
`faceauth-agent` (or run `/usr/libexec/faceauth-agent &`). Then:

```bash
faceauth doctor                                   # all checks OK (agent included)
grep -A1 faceauth-setup /etc/pam.d/{sudo,sudo-i,polkit-1}   # marked lines
systemd-analyze security faceauth-daemon          # about 0.6 SAFE
busctl introspect io.github.gvarsanyi.FaceAuth1 /io/github/gvarsanyi/FaceAuth1
systemctl status faceauth-daemon                  # active, started by D-Bus
sudo ls -ld /var/lib/faceauth                     # drwx------ faceauthd
```

## 3. Enrollment and management

- `faceauth cameras`: the IR camera is marked; no virtual devices.
- `faceauth add`: a password prompt appears in the terminal (polkit via
  pkttyagent); five captures, then success. Turn your head slightly between
  captures.
- `faceauth info`, `faceauth test` (exit status 0 when you look at the
  camera, non-zero when you look away).
- Cancel the polkit prompt (Ctrl+C or empty password): the command fails
  with "not authorized" and nothing changes.
- KDE System Settings, Face Authentication (and `faceauth-gui`): a graphical
  polkit dialog appears for enroll/delete/service toggles; test works; the
  enrollment progress bar moves; cancelling while the password dialog is
  open does not start the camera afterwards.
- GNOME or COSMIC: `faceauth-settings` behaves the same.
- `sudo ls -l /var/lib/faceauth`: `<uid>.enc.json` with mode 0600.

## 4. Authorization (negative tests)

Create a second, unprivileged account:

```bash
sudo useradd -m fatest && sudo passwd fatest
```

- `sudo -u fatest faceauth info -u $USER`: permission denied.
- `sudo -u fatest faceauth test -u $USER`: permission denied (and the
  camera light stays off).
- `faceauth add -u fatest` as yourself: asks for the *administrator*
  password (manage-other).
- Over SSH (`ssh localhost`), `faceauth test`: refused, not a local session.

Remove the account afterwards: `sudo userdel -r fatest`.

## 5. PAM

Use `sudo -k` before each `sudo` attempt to drop cached credentials.

- `sudo -k; sudo true` while looking at the camera: a notification appears
  and the command succeeds without a password.
- Look away: after the timeout (5 s) the password prompt appears; the
  password still works.
- `faceauth service disable sudo`, then `sudo -k; sudo true`: password
  prompt immediately, camera stays off. Re-enable with
  `faceauth service enable sudo`.
- **Root escalation regression:** add `su` to `services` in
  `/etc/faceauth/faceauth.conf`, run `sudo /usr/libexec/faceauth-setup
  configure`, then `su -` while looking at the camera: it must ask for root's
  password (root has no enrollment). Remove `su` again afterwards.
- SSH: `ssh -t localhost 'sudo -k; sudo true'`: password prompt, camera
  stays off.
- Background session: run `sleep 10; sudo -k; sudo true` in a terminal and
  switch to another VT (Ctrl+Alt+F3) within 10 s. The camera must stay off;
  back on the desktop the password prompt is waiting.
- polkit (socket-activated helper on polkit 126+): run `pkcheck
  --revoke-temp`, then `faceauth service disable sudo` (pkttyagent) or toggle
  a service in System Settings (graphical agent). Look at the camera: the
  journal shows `[polkit-1] starting` then `succeeded`, and the prompt closes
  by itself. `systemctl list-units 'polkit-agent-helper@*'` while a prompt is
  open shows the helper instance.
- Unplug the enrolled camera (if external), `sudo -k; sudo true`: password
  prompt; the daemon does not switch to another camera.
- Virtual cameras: `sudo modprobe vivid n_devs=1 vid_cap_nr=42` (or
  v4l2loopback), `sudo chmod 0666 /dev/video42`, then `tests/virtual-camera.sh
  42` prints PASS.
- `journalctl -u faceauth-daemon -b` shows one line per authentication
  attempt with user, service, result, and closest distance on failure.

## 6. Lock screen and notifications

- Meta+L while facing the screen: stays locked; the daemon logs "locked;
  face unlock waits until nobody is in view".
- Look away for more than 2 s and back: unlocks; the daemon logs
  "unlocked".
- Lock and leave the camera's view: the daemon logs "after a 5s back-off",
  then 10 s, then 20 s; the camera light is off between attempts.
- No notification appears for lock-screen attempts;
  `journalctl --user -b | grep faceauth-agent` shows a "started ... (screen
  locked; no notification)" and a result line for each.
- GNOME: add `gdm-password` to `services`, run `sudo
  /usr/libexec/faceauth-setup configure`; face unlock works on the lock
  screen, while logging in at GDM is refused by the daemon ("is a login
  screen").

## 7. Uninstall and packages

- `sudo make uninstall`: marked lines removed and the unchanged
  `/etc/pam.d` copies deleted (`ls /etc/pam.d`),
  `/var/lib/faceauth` deleted, files gone (`faceauth doctor` no longer
  exists), `/etc/faceauth/faceauth.conf` kept.
- Reinstall with `sudo make install`.
- Optional, with nfpm: `make rpm`, then `sudo make uninstall`,
  `sudo zypper install dist/*.rpm`, repeat steps 2-6 briefly, and
  `sudo zypper remove faceauth faceauth-kde faceauth-gtk` (face data is
  deleted on erase).

## 8. CI

Push a branch and check that all jobs of `.github/workflows/ci.yml`
(Ubuntu with the virtual-camera test, fuzz, openSUSE Tumbleweed) pass.
