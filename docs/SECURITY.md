# Security

## Trust model

gozler has the same trust model as Everything. Stated plainly:

- The daemon runs **elevated** and builds its index from a raw NTFS MFT read, or on ReFS from a folder walk with backup semantics, which both bypass per-file ACLs. The index therefore contains the name, size, and timestamps of every file on every tracked volume, regardless of who can normally read them.
- The query pipe (`\\.\pipe\gozler-v1`) grants **Authenticated Users** read/write, so any authenticated local user can query the index. It exposes only that metadata, never file contents. On a shared machine, treat filenames as visible to all local users.
- Conversely, the client verifies the pipe server's owner SID is Local System or the built-in Administrators group before sending anything, and fails closed. A non-elevated process cannot squat the pipe name and impersonate the daemon. The `--insecure-no-server-check` flag opts out of that check and is a development escape hatch only.
- A result snapshot, the rows of a search that the daemon keeps for later reads, is read by its id, 128 random bits that only the client that made it learns over its own pipe. Its rows are the same metadata any user can query. The daemon reads each client process's user SID only to cap how many result snapshots one user keeps, so one user's searches cannot fill the daemon's memory at another's expense beyond the cap on all of them.
- The Include settings, which decide what the index holds, are a value in `HKLM\SOFTWARE\Gozler`, which only administrators and Local System can write. Any user may send `SettingsSaved`, because the daemon then applies only what an administrator saved, so the pipe needs no check of who sends it.

## Reporting a vulnerability

Please report security issues privately rather than opening a public issue, using GitHub's [private vulnerability reporting](https://github.com/SynoSoftware/goz/security/advisories/new) (the "Report a vulnerability" button under the repository's Security tab). You will get an acknowledgement, and a fix or mitigation plan once the report is confirmed.
