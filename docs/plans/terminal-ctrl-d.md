# Plan: close the terminal panel with Ctrl+D (target 0.3.5)

Owner request (2026-11-16): the embedded terminal should close with Ctrl+D, as in Dolphin/Konsole.

Today Ctrl+D already ends the shell (EOF), but the panel stays open and shows "[Prozess beendet]" (`src/components/TerminalPanel.tsx:71-76`). The backend already sends the exit code (`TerminalExit {id, code}`, `src/ipc/api.ts:67`).

## Change
- In `TerminalPanel`'s exit handler: close the panel (`controller.toggleTerminal()` / dispatch `terminal: false`) when the shell exits — this covers Ctrl+D and typing `exit`.
- Exception so start errors stay readable: if the shell exits **within 2 s of opening with a non-zero code** (wrong shell, broken rc file), keep the panel open with the existing "[Prozess beendet]" line plus the exit code. Do not decide by exit code alone: bash returns the last command's status on Ctrl+D, so a failed last command would otherwise keep the panel open.
- Reopening (toolbar / shortcut) starts a fresh shell, as today.
- Ctrl+D is never intercepted by Dualfile: it goes to the shell, so programs inside the terminal (python, ssh, cat) keep their own Ctrl+D behaviour; only the shell's own exit closes the panel.

## Tests
- Vitest with the mock api: exit after > 2 s → panel closed; exit code 0 immediately → closed; non-zero exit within 2 s → stays open with message; exit of another terminal id → ignored.
- VM: Ctrl+D closes; `exit` closes; `python3` + Ctrl+D only leaves python; wrong shell path keeps the error visible.

Also add a line to the 0.3.5 release notes (DE/EN) and CHANGELOG.
