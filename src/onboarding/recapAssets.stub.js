// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

// Non-Android builds swap in this stub via the "@recap-assets" alias in
// vite.config.js, so the Android-only recap art (onboard-hero/mic/overlay/
// accessibility.png) never enters a desktop build's module graph at all.
// OnboardingWizard's recap steps only exist in androidStepSequence, so
// desktop never reads this.
export default null;
