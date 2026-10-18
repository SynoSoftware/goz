# gozler

gozler finds files and folders on NTFS volumes by name, path, size and date,
from an index of each volume that a daemon keeps current.

## Index

**Entry**:
One indexed name: a folder, or one name of a file.
_Avoid_: record, node

**Hard link**:
One of a file's names. Each hard link is its own entry; all of them share the
file's file ID, size and date.
_Avoid_: alias

**File ID**:
The NTFS number that identifies a file on its volume, shared by every hard link
of the file.
_Avoid_: FRN (outside code), inode

**Mount prefix**:
Where a volume is mounted, as gozler prints it: a drive such as `C:\`, or a
mount folder. A volume can have several; its full paths print with the first,
or with its GUID path when it has none.
_Avoid_: drive (for a volume mounted in a folder)

**Full path**:
The mount prefix followed by every folder and the entry's name, as gozler prints
it: `C:\repo\docs\testing.md`.
_Avoid_: path within the volume, relative path (when the drive is included)

**Folded name**:
An entry's name with case folded away; what a case-insensitive search and the
result order compare.

**Unknown size or date**:
A file's size or date that gozler did not read, because the size and date pass
failed or stopped, or could not read that file. A folder has no size, which is
not the same as an unknown size.

## Search

**Name term**:
A search term without a separator; it matches the entry's name.

**Path term**:
A search term with a `\` or `/`, or a `path:` value; it matches the full path,
as a substring.

**Scope**:
The folder `-path` names. A search finds the entries below it, on the volume
whose mount prefix is the longest that starts it.

**Path wildcard**:
A path term with `*` or `?`; it must match the whole full path, as `*\docs\*`
does.
_Avoid_: wildcard path term

**Result order**:
The order gozler prints matches in: by the sort key, then folded name, volume,
file ID, parent file ID and raw name, so the same query over the same index
prints the same list, and a limited list is the start of the full one.
