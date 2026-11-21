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
  // Cross-tab invalidation tests exercise BroadcastChannel, storage events,
  // and SSR (no-window) isolation. Run them in the jsdom environment, which
  // provides window/document, and let individual tests stub or delete
  // BroadcastChannel/localStorage to simulate unsupported browsers.
  projects: [
    {
      displayName: "jsdom",
      testEnvironment: "jest-environment-jsdom",
      testEnvironmentOptions: {
        customExportConditions: ["node", "node-addons"],
        url: "http://localhost/",
      },
      setupFilesAfterEnv: ["<rootDir>/jest.setup.ts"],
      testMatch: ["<rootDir>/src/**/*.test.{ts,tsx,js,jsx}"],
      testPathIgnorePatterns: [
        "<rootDir>/node_modules/",
        "<rootDir>/e2e/",
        "<rootDir>/.next/",
        "<rootDir>/src/**/*.ssr.test.{ts,tsx,js,jsx}",
      ],
      moduleNameMapper: {
        "^@/(.*)$": "<rootDir>/src/$1",
        "\\.(css|less|scss|sass)$": "<rootDir>/__mocks__/styleMock.js",
        "\\.(jpg|jpeg|png|gif|svg|webp)$": "<rootDir>/__mocks__/fileMock.js",
      },
      transform: {
        "^.+\\.(js|jsx|ts|tsx)$": ["babel-jest", { configFile: "./babel-jest.config.js" }],
      },
      transformIgnorePatterns: [
        "/node_modules/(?!(?:react-dnd|react-dnd-html5-backend|dnd-core)/|@react-dnd/)",
      ],
      globals: {
        NODE_OPTIONS: "--unhandled-rejections=warn",
      },
    },
    {
      // SSR isolation tests: no window/document/BroadcastChannel available.
      displayName: "node-ssr",
      testEnvironment: "node",
      testEnvironmentOptions: {
        customExportConditions: ["node", "node-addons"],
      },
      setupFilesAfterEnv: ["<rootDir>/jest.setup.ts"],
      testMatch: ["<rootDir>/src/**/*.ssr.test.{ts,tsx,js,jsx}"],
      testPathIgnorePatterns: [
        "<rootDir>/node_modules/",
        "<rootDir>/e2e/",
        "<rootDir>/.next/",
      ],
      moduleNameMapper: {
        "^@/(.*)$": "<rootDir>/src/$1",
        "\\.(css|less|scss|sass)$": "<rootDir>/__mocks__/styleMock.js",
        "\\.(jpg|jpeg|png|gif|svg|webp)$": "<rootDir>/__mocks__/fileMock.js",
      },
      transform: {
        "^.+\\.(js|jsx|ts|tsx)$": ["babel-jest", { configFile: "./babel-jest.config.js" }],
      },
      transformIgnorePatterns: [
        "/node_modules/(?!(?:react-dnd|react-dnd-html5-backend|dnd-core)/|@react-dnd/)",
      ],
      globals: {
        NODE_OPTIONS: "--unhandled-rejections=warn",
      },
    },
  ],
};