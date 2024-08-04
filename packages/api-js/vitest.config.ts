import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    typecheck: {
      enabled: true,
      checker: 'tsc',
      tsconfig: './tsconfig.json',
    },
    reporters: process.env.CI ? ['default', 'junit'] : 'default',
    outputFile: 'reports/junit.xml',
    coverage: {
      enabled: true,
      provider: 'v8',
      extension: ['ts'],
      reporter: process.env.CI ? ['json', 'json-summary'] : 'html',
      reportsDirectory: 'coverage',
    },
  },
})
