/** @type {import('jest').Config} */
module.exports = {
  setupFilesAfterEnv: ["<rootDir>/jest.setup.ts"],
  testEnvironment: "jest-environment-jsdom",
  testPathIgnorePatterns: [
    "<rootDir>/node_modules/",
    "<rootDir>/e2e/",
    "<rootDir>/.next/",
  ],
  // Collect coverage from the new realtime/offline/optimistic/redirect modules
  // as well as the existing client and data-boundary code.
  collectCoverageFrom: [
    "src/**/*.{ts,tsx,js,jsx}",
    "!src/**/*.d.ts",
    "!src/**/__tests__/**",
    "!src/**/*.test.{ts,tsx,js,jsx}",
    "!src/**/*.spec.{ts,tsx,js,jsx}",
  ],
  moduleNameMapper: {
    "^@/(.*)$": "<rootDir>/src/$1",
    "\\.(css|less|scss|sass)$": "<rootDir>/__mocks__/styleMock.js",
    "\\.(jpg|jpeg|png|gif|svg|webp)$": "<rootDir>/__mocks__/fileMock.js",
  },
  transform: {
    "^.+\\.(js|jsx|ts|tsx)$": ["babel-jest", { configFile: "./babel-jest.config.js" }],
  },
  // Ship ESM-only packages (react-dnd and its ecosystem) must be transformed
  // too — babel-jest compiles their `export` syntax to CommonJS.
  transformIgnorePatterns: [
    "/node_modules/(?!(?:react-dnd|react-dnd-html5-backend|dnd-core)/|@react-dnd/)",
  ],
  // Node 24 treats unhandled promise rejections as fatal by default.
  // Tests that intentionally throw async errors (e.g. ApiError tests) need
  // this flag so the jest worker process isn't killed before results are reported.
  testEnvironmentOptions: {
    customExportConditions: ["node", "node-addons"],
  },
  globals: {
    NODE_OPTIONS: "--unhandled-rejections=warn",
  },
  // Explicitly enumerate the test roots for the new feature areas so that
  // realtime sequencing, offline queue, optimistic reconciliation, and
  // return-destination validation suites are always discovered even if they
  // live outside the default `src` tree.
  roots: [
    "<rootDir>/src",
    "<rootDir>/__tests__",
  ],
  testMatch: [
    "<rootDir>/src/**/__tests__/**/*.{ts,tsx,js,jsx}",
    "<rootDir>/src/**/?(*.)+(spec|test).{ts,tsx,js,jsx}",
    "<rootDir>/__tests__/**/*.{ts,tsx,js,jsx}",
    "<rootDir>/__tests__/realtime/**/*.{ts,tsx,js,jsx}",
    "<rootDir>/__tests__/offline/**/*.{ts,tsx,js,jsx}",
    "<rootDir>/__tests__/optimistic/**/*.{ts,tsx,js,jsx}",
    "<rootDir>/__tests__/auth/**/*.{ts,tsx,js,jsx}",
  ],
};