# Playwright browser artifacts for @playwright/test 1.56.1.
#
# The web/ package pins `@playwright/test` to 1.56.1, whose bundled browsers.json
# asks for chromium/headless-shell revision 1194 (Chrome 141.0.7390.37) and
# ffmpeg revision 1011 (verified with `playwright install --dry-run chromium`
# against the installed node_modules). nixpkgs' own playwright-driver at this
# pin ships revision 1243, which Playwright 1.56.1 will NOT use, so we fetch the
# exact revisions from Playwright's own registry (the same dbazure URLs
# `playwright install` uses) rather than symlink a mismatched browser set.
#
# Layout mirrors what `playwright install` produces, so setting
# PLAYWRIGHT_BROWSERS_PATH to this derivation is all the e2e suites need:
#   chromium-1194/chrome-linux/chrome
#   chromium_headless_shell-1194/chrome-headless-shell-linux/headless_shell
#   ffmpeg-1011/ffmpeg-linux
#
# Only x86_64-linux is wired up here (the dev host and CI are x86_64). Other
# systems throw with a clear message instead of silently producing a broken tree.

{
  stdenv,
  lib,
  fetchzip,
  autoPatchelfHook,
  patchelf,
  makeWrapper,
  linkFarm,
  makeFontsConf,
  system,

  # runtime libraries the Chrome-for-Testing / headless-shell binaries dlopen
  alsa-lib,
  at-spi2-atk,
  atk,
  cairo,
  cups,
  dbus,
  expat,
  glib,
  libGL,
  libgbm,
  libgcc,
  libxkbcommon,
  nspr,
  nss,
  pango,
  systemd,
  vulkan-loader,
  pciutils,
  libxrandr,
  libxfixes,
  libxext,
  libxdamage,
  libxcomposite,
  libx11,
  libxcb,
  ...
}:

let
  throwSystem = throw "lessdugin playwright-browsers: unsupported system ${system}";

  chromiumRevision = "1194";
  ffmpegRevision = "1011";

  # Playwright's own download registry (matches `playwright install --dry-run`).
  builds = browser: rev: archive: "https://cdn.playwright.dev/dbazure/download/playwright/builds/${browser}/${rev}/${archive}";

  fontconfig_file = makeFontsConf { fontDirectories = [ ]; };

  chromeLibs = [
    alsa-lib
    at-spi2-atk
    atk
    cairo
    cups
    dbus
    expat
    glib
    libgbm
    libgcc
    libxkbcommon
    nspr
    nss
    pango
    stdenv.cc.cc.lib
    systemd
    libx11
    libxcomposite
    libxdamage
    libxext
    libxfixes
    libxrandr
    libxcb
  ];

  # The zip already contains a chrome-linux/ directory (Playwright's own layout),
  # so stripRoot = false and the derivation root IS the browser dir.
  chromium =
    if system == "x86_64-linux" then
      stdenv.mkDerivation {
        name = "playwright-chromium-${chromiumRevision}";
        src = fetchzip {
          url = builds "chromium" chromiumRevision "chromium-linux.zip";
          stripRoot = false;
          hash = "sha256-uf8FUMgUUyM1xw5eUuMUgUg9GIW8bDcNZz0mIfnoTLM=";
        };
        nativeBuildInputs = [ autoPatchelfHook patchelf makeWrapper ];
        buildInputs = chromeLibs;
        # Playwright expects `<browsersPath>/chromium-1194/chrome-linux/chrome`
        # (EXECUTABLE_PATHS.chromium.linux). fetchzip may or may not strip the
        # archive's single top-level `chrome-linux/` dir depending on its exact
        # contents, so normalize: if a `chrome-linux/` survived, copy its innards;
        # otherwise the browser files are already at `.`. Either way the result is
        # exactly `$out/chrome-linux/<files>`.
        installPhase = ''
          runHook preInstall
          mkdir -p $out/chrome-linux
          if [ -d chrome-linux ]; then
            cp -R chrome-linux/. $out/chrome-linux/
          else
            cp -R . $out/chrome-linux/
          fi
          chmod -R +w $out/chrome-linux
          test -x $out/chrome-linux/chrome
          wrapProgram $out/chrome-linux/chrome \
            --set-default SSL_CERT_FILE /etc/ssl/certs/ca-bundle.crt \
            --set-default FONTCONFIG_FILE ${fontconfig_file}
          runHook postInstall
        '';
        appendRunpaths = lib.makeLibraryPath [ libGL vulkan-loader pciutils ];
        postFixup = ''
          rm -f "$out/chrome-linux/libvulkan.so.1"
          ln -s -t "$out/chrome-linux" "${lib.getLib vulkan-loader}/lib/libvulkan.so.1"
        '';
      }
    else
      throwSystem;

  # The zip already contains a chrome-headless-shell-linux/ directory with a
  # headless_shell binary; keep stripRoot = false so the layout matches what
  # Playwright's findExecutablePath expects.
  chromiumHeadlessShell =
    if system == "x86_64-linux" then
      stdenv.mkDerivation {
        name = "playwright-chromium-headless-shell-${chromiumRevision}";
        src = fetchzip {
          url = builds "chromium" chromiumRevision "chromium-headless-shell-linux.zip";
          stripRoot = false;
          hash = "sha256-khYVM0jocno97lV8mRH71WHzopIjnq3eX/PD1kQuZnE=";
        };
        nativeBuildInputs = [ autoPatchelfHook patchelf ];
        buildInputs = [
          alsa-lib
          at-spi2-atk
          expat
          glib
          libxcomposite
          libxdamage
          libxfixes
          libxrandr
          libgbm
          libgcc
          libxkbcommon
          nspr
          nss
        ];
        # Playwright expects
        # `<browsersPath>/chromium_headless_shell-1194/chrome-linux/headless_shell`
        # (EXECUTABLE_PATHS["chromium-headless-shell"].linux). Normalize the same
        # way as the chromium derivation above so the result is exactly
        # `$out/chrome-linux/headless_shell`.
        installPhase = ''
          runHook preInstall
          mkdir -p $out/chrome-linux
          if [ -d chrome-linux ]; then
            cp -R chrome-linux/. $out/chrome-linux/
          else
            cp -R . $out/chrome-linux/
          fi
          chmod -R +w $out/chrome-linux
          test -x $out/chrome-linux/headless_shell
          runHook postInstall
        '';
      }
    else
      throwSystem;

  ffmpeg =
    if system == "x86_64-linux" then
      fetchzip {
        url = builds "ffmpeg" ffmpegRevision "ffmpeg-linux.zip";
        stripRoot = false;
        hash = "sha256-AWTiui+ccKHxsIaQSgc5gWCJT5gYwIWzAEqSuKgVqZU=";
      }
    else
      throwSystem;
in
linkFarm "playwright-browsers-1.56.1" [
  { name = "chromium-${chromiumRevision}"; path = chromium; }
  { name = "chromium_headless_shell-${chromiumRevision}"; path = chromiumHeadlessShell; }
  { name = "ffmpeg-${ffmpegRevision}"; path = ffmpeg; }
]
