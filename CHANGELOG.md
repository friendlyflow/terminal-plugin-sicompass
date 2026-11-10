# Changelog

## 0.3.3

- A second colon in the shell writes a command: the programs you have first, then the commands you ran before. Tab completes the word you are typing, and Enter puts the command in the input line, where Enter runs it. The input line no longer has the history under it.
- Needs sicompass 0.2.6.

## 0.3.2

- Output, commands and folder names are shown exactly as written, never read as markup, and a command runs with its own bytes.
- Needs sicompass 0.2.4.
- Intel macs are not built any more.

## 0.3.1

- The shell itself never opens the dashboard. Only a program running in the
  foreground does.
- fish no longer waits 10 seconds when it starts: the terminal now answers its
  device attributes query.

## 0.3.0

Terminal is a program of its own now, instead of a sandboxed WebAssembly component.
Sicompass starts it and talks to it, one per tab, and it runs with your rights, so
its entry in the Store says what it does before you install it, and installing it is
your approval.

- It runs your login shell, `$SHELL` first, or any shell you name in settings. The tab switcher shows the shell, and the tab says when a command is running.
- One build for each of Linux (x86_64 and arm64, static), macOS (Apple Silicon and
  Intel) and Windows.
- Needs a Sicompass that runs plugin programs. An older Sicompass keeps the 0.2
  version it has.
