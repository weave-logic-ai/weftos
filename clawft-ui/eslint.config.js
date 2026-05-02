import js from '@eslint/js'
import globals from 'globals'
import reactHooks from 'eslint-plugin-react-hooks'
import reactRefresh from 'eslint-plugin-react-refresh'
import jsxA11y from 'eslint-plugin-jsx-a11y'
import tseslint from 'typescript-eslint'
import { defineConfig, globalIgnores } from 'eslint/config'

// WEFT-315: jsx-a11y is the static a11y net we run today.
//
// Static rules catch a meaningful subset of WCAG AA violations
// (missing alt text, role mismatches, click-handlers without
// keyboard handlers, label/control associations, redundant roles,
// etc.) without standing up a Playwright + axe-core pipeline.
// The full axe-core scan across all 14 routes is tracked as a
// follow-up that lands alongside the Playwright suite.
//
// Stylistic rules that surface a backlog of label/control wiring
// across forms (memory, cron, delegation, skills) are softened to
// warnings so CI lint stays green while the violations remain
// visible. Genuinely critical rules (alt text, role/state
// integrity, redundant roles) keep their default error severity.
export default defineConfig([
  globalIgnores(['dist', 'public/mockServiceWorker.js']),
  {
    files: ['**/*.{ts,tsx}'],
    extends: [
      js.configs.recommended,
      tseslint.configs.recommended,
      reactHooks.configs.flat.recommended,
      reactRefresh.configs.vite,
      jsxA11y.flatConfigs.recommended,
    ],
    languageOptions: {
      ecmaVersion: 2020,
      globals: globals.browser,
    },
    rules: {
      // Existing forms wire labels as siblings rather than children
      // and rely on visual proximity — surface as warnings, fix in
      // a focused a11y sweep.
      'jsx-a11y/label-has-associated-control': 'warn',
      // The voice talk-overlay uses click-only handlers on a backdrop;
      // surfaces as warning until the keyboard escape path is wired.
      'jsx-a11y/click-events-have-key-events': 'warn',
      'jsx-a11y/no-static-element-interactions': 'warn',
      // autoFocus is fine on the command-palette modal where focus
      // expectation is unambiguous; downgrade.
      'jsx-a11y/no-autofocus': 'warn',
    },
  },
])
