# Security

## Trust model

gozler has the same trust model as Everything. Stated plainly:

- The daemon runs **elevated** and builds its index from a raw NTFS MFT read, or on ReFS from a folder walk with backup semantics, which both bypass per-file ACLs. The index therefore contains the name, size, and timestamps of every file on every tracked volume, regardless of who can normally read them.
- The query pipe (`\\.\pipe\gozler-v1`) grants **Authenticated Users** read/write, so any authenticated local user can query the index. It exposes only that metadata, never file contents. On a shared machine, treat filenames as visible to all local users.
- Conversely, the client verifies the pipe server's owner SID is Local System or the built-in Administrators group before sending anything, and fails closed. A non-elevated process cannot squat the pipe name and impersonate the daemon. The `--insecure-no-server-check` flag opts out of that check and is a development escape hatch only.
- The Include settings, which decide what the index holds, are a value in `HKLM\SOFTWARE\Gozler`, which only administrators and Local System can write. Any user may send `SettingsSaved`, because the daemon then applies only what an administrator saved, so the pipe needs no check of who sends it.

## Reporting a vulnerability

Please report security issues privately rather than opening a public issue, using GitHub's [private vulnerability reporting](https://github.com/SynoSoftware/goz/security/advisories/new) (the "Report a vulnerability" button under the repository's Security tab). You will get an acknowledgement, and a fix or mitigation plan once the report is confirmed.
