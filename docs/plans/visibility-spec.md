# Spec: show each user only what they may see

What gozlerd shows to each user of a PC, and why. This spec changes only when
a requirement changes. [The plan](visibility-plan.md) says how it is built,
in what order, and how each stage proves that it is done.

Each requirement has an ID, such as `V3`, and a "Checked by" entry that names
the checks in the plan that prove it.

## Goal

Today gozlerd shows every user every name on the PC. It reads the MFT, or on
ReFS walks the folders with backup semantics, so its index holds every name,
size and date, whatever the file permissions say. The pipe grants
Authenticated Users read and write (`docs/SECURITY.md`). On a shared PC, ben
can search for `C:\Users\ana\Documents\*` and read the names of ana's files,
their sizes and their dates. A folder size shows how much ana keeps there.

After this spec, a user finds an entry only when File Explorer would show
them its name, and every count and total gozler gives them leaves out what
they cannot see. The exception is a list of users and groups who see every
file. The setup puts the user who installs gozler on it, so on a PC with one
person nothing changes. An administrator can add other users or groups, or
Everyone, which turns the filter off.

The Include page that [the service settings plan](service-settings.md#the-include-page) holds
does not solve this. It lets an administrator leave folders out of the index
for everyone. It cannot let ana find her own files while it hides them from
ben.

## Owner decisions

**Settled** (the owner, 2026-10-18):

- gozlerd filters what it shows by user, and the filter is on by default.
  `docs/DESIGN.md` lists "Results filtered by the caller's file permissions"
  as not planned; this spec replaces that line.
- The filter can be turned off. The owner's example: a person who works as a
  standard user on their own PC, for safety, and wants to see every file
  without running as administrator. Letting chosen users or groups see
  everything is wanted too, if it is not complicated. It is not: one list does
  both (`V16`).
- One stored visibility policy: the list of users and groups who see every
  file, by SID. There is no separate on/off setting. Adding Everyone turns
  the filter off, and a check box in the window is only a way to add or remove
  Everyone.
- The setup offers the signed-in user a place on the list, with the box
  already checked, and shows it before anything is granted, so the
  administrator who approves the install can clear it (`V17`).
- gozler never invents its own folder permissions. Windows' NTFS
  permissions decide what each user sees, and the list is the only
  exception.
- **Which names a user sees:** the names in the folders the user may list,
  as File Explorer shows them on a local disk (`V1`).
- **No administrator exception.** An administrator sees what their token may
  list, like any user, unless the list names them. The user who installs
  gozler sees every file, also without elevation, because the setup puts
  them on the list (`V17`). Other users and groups see every file only when
  they are added to the list (`V16`).
- **A user learns only about files they may see.** Counts and totals add only
  visible files: 10 visible files of 500 MB in total give 500 MB, and a
  hidden file never counts, because counting it would reveal it. A user who
  does not see every file gets no folder sizes, because keeping a
  visible-only size for every folder and every such user would slow every
  search (`V6`).

**Defaults this spec sets.** Each has only one reasonable answer:

- **Machine-wide status.** `--status`, `Hello` and the window's Index page
  give whole-volume counts. A count of entries shows no name, size or date,
  and counting per user would need a full scan for each status request.
- **No anonymous searches.** A client that does not let gozlerd see which
  user it runs as gets no search results (`V11`). If gozlerd served such a
  client as nobody, it would get empty answers that look like "no such file".
  If gozlerd served it everything, any program could bypass the filter.

## Who sees which names

**V1. A user sees an entry when they may list the folder that holds it.**
"May list" means that the folder's security descriptor grants the user's
token `FILE_LIST_DIRECTORY`, as `AccessCheck` decides it. A user who can
open a folder sees every name in it, including the names of files they
cannot open, as in File Explorer. Each
hard link is its own entry, so a file with names in two folders shows under
the folder the user can list and not under the other one. The volume's root
folder has no folder above it, so every user sees it.
Checked by: plan, stage 2 checks 4 and 5; tests T5 and T6.

**V2. A full path can name folders the user cannot list.** A row's full path
contains every folder above the entry. When a folder deep in a tree is shared
with a user, its entries show with their full path, even when the user cannot
list a folder above it. Windows itself shows those names to a user who types
the path, because every user has "Bypass traverse checking" by default.
Checked by: plan, stage 2 check 5.

**V3. Who sees every name.** A request sees every name when its token's user
or one of its enabled groups is on the list (`V16`), or when it runs as
SYSTEM. SYSTEM is the operating system's own account, which can already read
every folder, and its callers are tasks and services that work for the whole
PC, not people searching. No other account sees every name without the list:
an administrator, elevated or not, and a service account such as Local
Service or Network Service see what their token may list (`V4`).
Checked by: plan, stage 2 check 3; test T9.

**V4. Everyone else sees what their token may list.** The token's user, its
groups, its restricted SIDs and its integrity level decide, as they decide
when that user opens a folder.
Checked by: plan, stage 0 item 4; stage 2 checks 4 and 5.

**V16. An administrator decides who else sees every file.** The service keeps
one list, "Users and groups who see every file". The setup fills it on the
first install (`V17`). A request whose user, or one of whose enabled groups,
is on the list sees every name (`V3`). A group that the token holds only for
deny, such as Administrators in a non-elevated administrator's token, does
not count, so adding Administrators lets administrators see every file only
when they run elevated.

- Adding one's own account is the owner's case: a standard user sees every
  file on their own PC without running as administrator.
- Adding **Everyone** turns the filter off for every user.
- Adding a group lets its members see every file.

Only an administrator changes the list, because a user who could change it
could turn the filter off for themselves. Anyone may read it. A change
applies from the next search, with no restart. Until the window's Index page
offers the list (`V14`), an elevated `gozlerd see-every-file` command shows,
adds and removes entries by account name or SID. Without `add` or `remove` it
only shows the list, and it needs no elevation for that; it also says how to
show every file to every user. `Everyone` works on Windows in any language.
Uninstalling the service drops the list, so after a new install the filter is
on again; an upgrade that only replaces the programs keeps it.
Checked by: plan, stage 2 check 9; test T9.

**V17. The setup offers the signed-in user.** The signed-in user is the user
of the Windows session that runs the setup. It is not the account that
approved the UAC prompt. When a standard user starts the setup and another
administrator types their password, the setup runs as that administrator, but
the signed-in user is still the standard user, and that is the person the
setup offers.

- On a first install, when no list is stored yet, the setup shows one page
  with one check box, checked: "Let PC\ana see the names of every file on
  this PC". The page says that this lets that user see every file name
  without running as administrator, and that it does not let them open a file
  they cannot open now.
- Checked, the setup stores the list with that user. Cleared, it stores an
  empty list. Either way the choice is stored before the service answers any
  search with the new rules.
- The setup shows no page, and adds nobody, when:
  - a list is already stored, as on an upgrade, so that a user an
    administrator removed is never added back;
  - it runs silently (`/SILENT` or `/VERYSILENT`), because nobody saw the
    choice;
  - no user is signed in to its session, as when a deployment tool runs it
    as SYSTEM.
- The page shows for an administrator too, because an administrator sees
  every file only through the list (`V3`).
- Uninstalling drops the list (`V16`), so the next install shows the page
  again.
- `gozlerd install`, run by hand or by the window's "Install the service"
  button, adds nobody unless it is told whom to add (plan, "The setup's
  choice").

Checked by: plan, stage 0 item 8; stage 2 check 11.

## What every answer leaves out

**V5. Every answer counts the same entries.** For a user who cannot see
everything, each of these leaves out the entries that `V1` hides:

- the rows of a search, its `total` and the "shown of total" warning;
- `-get-result-count`, `-get-total-size` and `-get-folder-size`;
- `-export-*` files, which take the same rows;
- a folder's size, which such a user does not get (`V6`);
- every search, whatever its form: a name term, a path term, a wildcard, a
  regular expression, a `-path` scope, a protocol 1 text query.

Checked by: plan, stage 2 checks 4 and 6; tests T6 and T8.

**V6. Only a user who sees every file gets folder sizes.** For every other
user, a folder's size is unknown, as it is today on a volume whose size pass
has not finished: the size column and `--json` show none, `size:` never
matches a folder, and the size sort places folders as it places other
unknown sizes. Files keep their sizes. `-get-folder-size` and
`-get-total-size` still work, because they add the sizes of the visible
files at the time of the search: 10 visible files of 500 MB in total give
500 MB, whatever is hidden beside them. An unknown folder size here is the
rule, not a missing pass, so it never causes exit 2 (`V7`).
The reason: a visible-only size for each folder would have to be kept for
each user who does not see every file, at the cost of megabytes per user, of
work under each volume's write lock on every size change, and of a pass
over every entry when such a user first searches, while searches on that
volume wait. Fast searching matters more than folder sizes for these users.
Checked by: plan, stage 2 check 6; test T6.

**V7. A filtered answer is complete.** Leaving out entries that the user may
not see does not make an answer incomplete. It never causes exit 2 or a
warning. Exit codes 9 and 10 count only the entries the user sees.
Checked by: plan, stage 2 check 4.

## When permissions change

**V8. A permission change shows as fast as a rename.** When a folder's
permissions change, a search sees the change within a second, as it sees a
new or renamed file. This includes the folders that inherit the change. A new
folder's entries show to the users who may list it as soon as gozlerd has
read the folder's permissions, which happens in the same journal batch that
adds the folder.
Checked by: plan, stage 0 item 3; stage 1 check 4; stage 2 check 5.

**V9. Until gozlerd has read a folder's permissions, the folder's entries
are hidden from every user who does not see everything.** This applies to a
new folder and to a folder whose permissions changed: from the journal batch
that records the folder or the change, until gozlerd has read the folder's
permissions, which it does first in that batch, so this lasts only
milliseconds. A folder that loses a user's permission thus never shows that
user a name added after the change. When a read fails, gozlerd tries again at
the folder's next change, `--status` shows how many reads have failed since
the volume was last scanned, and the next rescan reads every folder again.
Hiding is the safe default: showing the entries could show a name to a user
who may not see it.
Checked by: plan, stage 1 checks 2 and 4; tests T4 and T6.

## Pipe and command line

**V10. A search needs to know who is asking.** gozlerd identifies the user
of each search from the pipe. Every client must open the pipe at the
identification impersonation level or higher. At this level gozlerd can read
the caller's user and groups, but it cannot act as the caller. `gozler` and
the window use this level from this change on. They still check that the pipe
belongs to SYSTEM or Administrators before they send anything, so a program
that took the pipe name never learns who connected.
Checked by: plan, stage 2 check 7; test T8.

**V11. A search from an unknown caller is refused, with a reason.** A search
from a client that connects anonymously gets `Response::Error` with
`ProtoError::Unsupported` and the message `gozlerd could not tell which user
is searching; open the pipe at the identification level`. When Windows fails
to give gozlerd the caller's token for another reason, the search gets the
same error with the message `gozlerd could not tell which user is searching`.
A `gozler` built before this change prints the message and exits 7, its code
for `Unsupported`. `Hello` and `Status` still answer, because they show no
name. This breaks third-party programs that open the pipe anonymously, until
they change how they open it.
Checked by: plan, stage 2 check 7; test T8.

**V12. The payloads do not change.** No request, reply or results page
gains or loses a field for filtering, so `PROTO_VERSION` stays. A newer
client that talks to an older gozlerd gets every name, as today; `gozler` and
`gozlerd` install together (`docs/DESIGN.md`, "The version gate").
`VolumeStatus` gains the count of failed permission reads, and `VolumeMemory`
gains the memory that permissions use. Both have `#[serde(default)]`, so an
older peer still reads the status.
Checked by: plan, stage 1 check 2.

**V13. The help says it once.** The full reference gains one note: "A search
shows the files in folders you can open, and folder sizes only if you see
every file. The users and groups that `gozlerd see-every-file` lists see
every file." The quick help does not
change. `docs/WRITING.md` governs the final wording.
Checked by: plan, stage 2 check 8.

## Beside the Include page

**V14. Include decides the index; visibility decides each user's view of
it.** The Include page leaves folders out of the index for every user. This
spec decides which indexed entries each user sees. An excluded folder is in
no user's results. An included one shows to the users `V1`, `V3` and `V16`
allow. Two things follow for the service settings plan, which is not written
yet:

- The Include page's preview line counts the whole index, not the caller's
  view, because only an administrator changes that page and its change
  reaches every user.
- The Index page shows "Users and groups who see every file" as a list with
  Add and Remove, where Add opens Windows' "Select Users or Groups" dialog,
  the one Windows' own permission editors use. Above the list, a check box
  "Every user sees every file" is checked when the list holds Everyone;
  checking it adds Everyone and clearing it removes Everyone. It stores
  nothing of its own. The page reads and writes the same stored list as
  `gozlerd see-every-file`, and it needs administrator permission to change
  it, as the other service settings do.

Checked by: the service settings plan.

## ReFS

**V15. ReFS follows the same rules.** ReFS gives no `SecurityId`. gozlerd
reads each folder's permissions while it walks the folder, and the journal
reports a permission change on ReFS as it does on NTFS.
Checked by: plan, stage 0 items 2 and 3; stage 1 check 4.

## Not covered

- **How long a query takes.** A query that matches many hidden entries takes
  longer than one that matches none. gozlerd does not pad response times,
  because no name can be read from a duration.
- **Central access policies.** On a domain that uses Dynamic Access Control,
  Windows also applies central access policies and claims when a user opens
  a folder. gozlerd checks the folder's descriptor against the token only, so
  on such a domain its answer can differ from Explorer's. gozler targets
  desktops, and the owner's PCs have no domain.
- **Names above a shared folder** (`V2`): Windows shows them too.
