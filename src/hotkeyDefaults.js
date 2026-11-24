// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

import { IS_LINUX } from "./platform.js";

// Command mode's default hotkey, mirroring config.rs. Windows and macOS keep
// the same short two-modifier shape as dictation's Ctrl+Win. Linux needs a real
// key (Super belongs to the compositor, and a bare chord can't be bound there).
export const DEFAULT_COMMAND_HOTKEY = IS_LINUX ? "Shift+Alt+C" : "Alt+Win";

export const sameCombo = (a, b) =>
  String(a || "").trim().toLowerCase() === String(b || "").trim().toLowerCase();

// What command mode is set to right now, with the default when it isn't saved.
export const commandHotkeyOf = (config) => config?.command_hotkey || DEFAULT_COMMAND_HOTKEY;

export const commandModeOn = (config) => config?.command_mode_enabled !== false;
