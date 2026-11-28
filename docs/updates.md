# Updates

Back to the [README](../README.md).

Juicetray checks [GitHub Releases](https://github.com/ittmchien/juicetray/releases) for a newer
version at most once a day (restarting the app does not check again). Turn it off
with **Check for updates automatically** under **Settings**, or check by hand with **Check for
updates…**. The request sends nothing but the version in the User-Agent (`juicetray/<version>`).

On Windows, while **Detect new devices** is on, the "New device found" popup for a device the
built-in catalogue does not know first downloads the public catalogue files from the `main` branch
on GitHub, until one lists it. The device ID is not sent; the request carries only
`juicetray/<version>` in the User-Agent. Turning off **Detect new devices** stops it. A catalogue row reaches users once it is
merged to `main`, without a new release.

On Windows, **Update now** downloads the new exe, checks its SHA-256 against the release's
`SHA256SUMS.txt`, swaps it in and restarts; if anything fails, the old exe stays in place. When
the exe's folder is not writable it opens the download page instead.

On macOS, when Juicetray was installed with Homebrew, **Update now** runs
`brew upgrade ittmchien/tap/juicetray`, replaces `/Applications/Juicetray.app` with the new copy
(staged next to it first, so a failed copy leaves the old app in place) and restarts. When it runs
from Homebrew's own folder instead, it restarts from there and offers the move to Applications. If
anything fails, a dialog says why and the running version stays. Without Homebrew it offers the
download page instead.

While **Update now** runs, a notification says it started and the top of the menu shows the
current step (*Downloading…* or *Updating … via Homebrew…*, then *Installing…*, then the app restarts);
**Check for updates…** is greyed out until it finishes.

To update macOS by hand: quit Juicetray, run `brew upgrade juicetray`, open the app again and choose **Move** to replace the copy in Applications.
