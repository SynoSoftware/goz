# Writing the help and messages

This document says how gozler's words read: the help text in
`gozler_core::escompat::help` and in the help column of its tables, the
`gozler: ` lines on stderr, and the meaning of each exit code. The "Command line"
and "Exit codes" sections of `DESIGN.md` say what the help contains, how it is
split into two screens and which failure exits with which code.

## The direction

**Owner decision: plain, short and task-first, in the Microsoft Fluent 2 tone.**
A person or an agent reads the help or a message to do something now, so the
text says what to do in common words, and nothing more.

- **Lead with the task.** The quick help opens with examples of common tasks,
  and each example starts with what it does: "Find .cs files", "Show the 10
  largest .iso files". A reader finds the task first and the syntax second.
- **True before polished.** Check every statement against what gozler does, by
  running it in the shells it names. When the text and the behavior disagree,
  fix the text, unless the behavior is the defect. A wrong help that reads well
  costs a user more than a plain one that is right.
- **Fix the words, do not add more.** A correction replaces a phrase; it does
  not add a section. Detail that does not fit one line becomes a note in the
  full reference, as `DESIGN.md` says. A longer help hides the common tasks.
- **Write a list in parallel form.** Every example starts with a verb, such as
  Find, Show or Save, and says what the command does. A second idea gets its
  own sentence, not parentheses, because a reader scans a list by its first
  words.
- **Use one word for one thing.** The same option, state or folder has the same
  name on every screen and in every message, as in `CONTEXT.md`. A second name
  makes a reader ask whether it is a second thing.

## Messages

A message that reports a problem has three parts, in this order:

1. `gozler: ` and what is wrong, in one short line.
2. What the next lines do, such as "To start it, open a terminal as
   administrator and run:".
3. Each command alone on its own line, indented two spaces, with nothing else on
   that line, so that a copy takes exactly the command.

```
gozler: the gozler service is stopped
To start it, open a terminal as administrator and run:
  net start gozler
```

- **Every command works as pasted.** Trace what happens when the reader pastes
  it into cmd, Windows PowerShell 5.1 and PowerShell 7, in an administrator
  terminal, which opens in System32. So a message writes `sc.exe`, because
  PowerShell reads `sc` as `Set-Content`; puts two commands on two lines,
  because Windows PowerShell 5.1 has no `&&`; and uses `pushd` with the quoted
  folder of the `gozler.exe` that is running, because that folder is not always
  on `PATH` and `pushd` also changes the drive in cmd. A command that fails when
  pasted turns one problem into two.
- **Say the state the reader can check.** Write "the gozler service is not
  installed" or "is not ready; try again shortly", the states Windows shows in
  Services. Leave out how gozler found it out, such as which folder lacks
  gozlerd.exe: a reader who does not know that gozlerd.exe belongs beside
  gozler.exe reads that as a bug, and the line below it already says what to
  do.
- **Show a state as a word, not a symbol.** In plain text a circle or a dot
  reads as a bullet point. Color may mark the state too, but the word carries
  it, so a file, a pipe or a color-blind reader loses nothing.
- **Point to the next step only where it helps.** An unknown option names
  `--help`; a missing `gozlerd.exe` names the releases page. A message
  with no step the reader can take ends after its first line.

## Exit codes and quoting

- **An exit code's meaning is one sentence that a script can act on.** It says
  what the caller can rely on, as in "The result limit left out some matches"
  for 10. A script reads the code, and a person reads the sentence to write that
  script.
- **Separate what gozler receives from what the shell needs.** The search syntax
  lists the text gozler receives. The quoting rows show, for each shell by name,
  how to type that text so the shell passes it unchanged. A reader who mixes the
  two quotes the wrong layer, and the search changes without an error.

## Color

The overview and both help screens color text by its color token,
`gozler_core::output::style::ColorToken`, never by a color chosen for one line.
One meaning then has one color everywhere, and a new screen picks a meaning,
not a color.

| Token | Color | Used for |
| --- | --- | --- |
| Strong | bold | gozler's name and version, section titles |
| Muted | gray | row labels, such as Service |
| Primary | cyan | what the reader types: commands, options, search terms |
| Success | green | gozlerd runs; exit 0 |
| Warning | yellow | gozlerd is not ready; the answer may be incomplete or cut: exits 2, 9 and 10 |
| Danger | red | gozlerd does not run; the command failed: exits 1 and 3 to 8 |

`ExitCode::token` decides each exit code's token, so the quick help and the
full reference show a code in one color.
