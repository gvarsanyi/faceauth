# Upstream proposals

Patches for other projects, kept here until they are submitted and merged.

- `pam-config-add-faceauth.patch`: adds `pam-config --add --faceauth` to
  openSUSE's [pam-config](https://github.com/SUSE/pam-config). Its
  `pam-config.d` snippets can only override built-in modules, so faceauth
  cannot be enabled through pam-config without this. Licensed GPL-2.0 like
  pam-config itself. Once released, `faceauth-setup` can use pam-config on
  openSUSE instead of editing `/etc/pam.d/<service>` files.
  Status: written against pam-config 7ab3c20; not yet compiled or submitted.

  Build and try it:

  ```bash
  git clone https://github.com/SUSE/pam-config && cd pam-config
  git apply /path/to/faceauth/packaging/upstream/pam-config-add-faceauth.patch
  ./autogen.sh && ./configure && make
  mkdir -p /tmp/conf/pam.d
  src/pam-config --confdir=/tmp/conf --create
  src/pam-config --confdir=/tmp/conf --add --faceauth --faceauth-timeout=7
  grep faceauth /tmp/conf/pam.d/common-auth-pc
  ```
