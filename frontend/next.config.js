let withPWA = (config) => config;
try {
  withPWA = require("next-pwa")({
    dest: "public",
    register: true,
    skipWaiting: true,
    disable: process.env.NODE_ENV === "development",
  });
} catch {
  console.warn("[next.config] next-pwa unavailable, running without PWA");
}

// next-intl App Router plugin: wires ./src/i18n/request.ts so `next build`
// can resolve the request config (without this, builds fail with
// "Couldn't find next-intl config file").
let withNextIntl = (config) => config;
try {
  const createNextIntlPlugin = require("next-intl/plugin");
  withNextIntl = createNextIntlPlugin("./src/i18n/request.ts");
} catch {
  console.warn("[next.config] next-intl plugin unavailable, running without i18n");
}

// Content-Security-Policy is no longer set here (#1091): a static policy
// can't carry a per-request nonce, so it had to allow 'unsafe-inline'
// 'unsafe-eval' in script-src — defeating CSP's script protection entirely.
// `middleware.ts` now generates a nonce per request and sets the CSP header
// itself (see `src/lib/csp.ts`).

/** @type {import('next').NextConfig} */
const nextConfig = {
  // ESLint and TypeScript build checks
  eslint: {
    ignoreDuringBuilds: true,
  },
  typescript: {
    ignoreBuildErrors: true,
  },
  staticPageGenerationTimeout: 180,
  // Image optimization for mobile
  images: {
    formats: ["image/avif", "image/webp"],
    deviceSizes: [320, 420, 768, 1024, 1200, 1920, 2048],
    imageSizes: [16, 32, 48, 64, 96, 128, 256, 384],
    minimumCacheTTL: 60 * 60 * 24 * 30,
    dangerouslyAllowSVG: true,
    contentSecurityPolicy: "default-src 'self'; script-src 'none'; sandbox;",
    remotePatterns: [
      { protocol: "https", hostname: "api.dicebear.com", pathname: "/**" },
      { protocol: "https", hostname: "**" },
    ],
    loader: "custom",
    loaderFile: "./src/lib/imageLoader.ts",
  },
  compress: true,
  // Generate client source maps so production errors have readable stacks and
  // can be uploaded to Datadog (Issue #1100). The upload runs as `postbuild`.
  productionBrowserSourceMaps: true,
  experimental: {
    optimizePackageImports: [
      "lucide-react",
      "framer-motion",
      "recharts",
      "@tanstack/react-query",
      // Heavy feature modules (#247): barrel imports are rewritten to
      // per-symbol imports so only the used code lands in the route chunk.
      "@stellar/stellar-sdk",
    ],
  },

  // Webpack configuration for code splitting
  //
  // ── Server/client boundary for heavy feature modules (#247) ─────────────
  // Governance, analytics and game-only modules are *client* code. They must
  // never be imported from a Server Component, middleware, or any module that
  // runs on the server: private/financial state stays server-authoritative and
  // is fetched through the API boundary, never bundled into these chunks.
  //
  // On the client, these modules are only reachable via `next/dynamic`
  // (or an equivalent lazy import) from the route/interaction that needs them.
  // The cache groups below isolate them into async-only chunks: `chunks:
  // 'async'` plus `enforce: true` guarantees webpack will not hoist them into
  // the shared entry chunk, so public entry routes never download them.
  // ────────────────────────────────────────────────────────────────────────
  webpack: (config, { isServer }) => {
    // Split chunks for better caching
    if (!isServer) {
      config.optimization = {
        ...config.optimization,
        splitChunks: {
          chunks: 'all',
          cacheGroups: {
            default: false,
            vendors: false,
            // ── Heavy feature chunks (#247) ──────────────────────────────
            // Async-only: these groups are restricted to `chunks: 'async'`
            // and `enforce: true`, so they are emitted as separate lazy
            // chunks and never merged into the shared entry bundle.
            featureGovernance: {
              name: 'feature-governance',
              chunks: 'async',
              test: /[\\/]src[\\/](features|components|app)[\\/](governance|proposals|voting|dao)[\\/]/,
              priority: 45,
              enforce: true,
              reuseExistingChunk: true,
            },
            featureAnalytics: {
              name: 'feature-analytics',
              chunks: 'async',
              test: /[\\/]src[\\/](features|components|app)[\\/](analytics|charts|insights|metrics)[\\/]/,
              priority: 45,
              enforce: true,
              reuseExistingChunk: true,
            },
            featureGame: {
              name: 'feature-game',
              chunks: 'async',
              test: /[\\/]src[\\/](features|components|app)[\\/](game|games|play|arena)[\\/]/,
              priority: 45,
              enforce: true,
              reuseExistingChunk: true,
            },
            // Chart libraries are only used by analytics/game routes; keep
            // them async-only so entry routes don't pay for recharts/d3.
            charts: {
              name: 'charts',
              chunks: 'async',
              test: /[\\/]node_modules[\\/](recharts|d3-)[\\/]/,
              priority: 33,
              enforce: true,
            },
            // Framework chunks
            framework: {
              name: 'framework',
              chunks: 'all',
              test: /[\\/]node_modules[\\/](react|react-dom|scheduler)[\\/]/,
              priority: 40,
              enforce: true,
            },
            // UI library chunks
            ui: {
              name: 'ui',
              chunks: 'all',
              test: /[\\/]node_modules[\\/](@radix-ui|lucide-react|framer-motion)[\\/]/,
              priority: 35,
              enforce: true,
            },
            // Data fetching chunks
            data: {
              name: 'data',
              chunks: 'all',
              test: /[\\/]node_modules[\\/](@tanstack|react-query)[\\/]/,
              priority: 34,
              enforce: true,
            },
            // Stellar SDK chunks — async-only (#247): only wallet/transaction
            // interactions need the SDK, so it must not sit in the entry chunk.
            stellar: {
              name: 'stellar',
              chunks: 'async',
              test: /[\\/]node_modules[\\/](@stellar)[\\/]/,
              priority: 32,
              enforce: true,
            },
            // Library chunks
            lib: {
              test: /[\\/]node_modules[\\/]/,
              name(module) {
                const packageName = module.context.match(/[\\/]node_modules[\\/](.*?)([\\/]|$)/)[1];
                return `lib.${packageName.replace('@', '')}`;
              },
              priority: 30,
              minChunks: 2,
              reuseExistingChunk: true,
            },
            // Common chunks
            commons: {
              name: 'commons',
              chunks: 'all',
              minChunks: 2,
              priority: 20,
            },
          },
        },
        // Module IDs for better long-term caching
        moduleIds: 'deterministic',
        // Runtime chunk for better caching
        runtimeChunk: 'single',
      };
    }
    return config;
  },
  async redirects() {
    return [
      {
        // Block public access to source map files (#50)
        source: "/:path*.map",
        destination: "/404",
        permanent: false,
      },
    ];
  },
  // Bundle budget regression check (#247): `npm run build` writes the
  // bundle-analysis artifact, and `scripts/check-bundle-budget.mjs` fails the
  // build when a route exceeds its documented transfer/LCP budget.
  async rewrites() {
    const apiUrl = process.env.NEXT_PUBLIC_API_URL;
    if (apiUrl) {
      return [
        {
          source: "/api/:path*",
          destination: `${apiUrl}/api/:path*`,
        },
      ];
    }
    return [];
  },
  async headers() {
    return [
      {
        // Apply security headers to all routes
        source: "/:path*",
        headers: [
          // ── Security headers ───────────────────────────────────────────
          // Content-Security-Policy is set per-request by middleware.ts,
          // which needs a fresh nonce for every response (#1091).
          {
            // Prevent the page from being embedded in a frame (clickjacking)
            key: "X-Frame-Options",
            value: "DENY",
          },
          {
            // Prevent MIME-type sniffing
            key: "X-Content-Type-Options",
            value: "nosniff",
          },
          {
            // Control referrer information sent with requests
            key: "Referrer-Policy",
            value: "strict-origin-when-cross-origin",
          },
          {
            // Disable unused device capabilities
            key: "Permissions-Policy",
            value: "camera=(), microphone=(), geolocation=(), interest-cohort=()",
          },
          {
            // Legacy XSS filter for older browsers
            key: "X-XSS-Protection",
            value: "1; mode=block",
          },
          {
            // Enforce HTTPS (2 years, include subdomains, preload-ready)
            key: "Strict-Transport-Security",
            value: "max-age=63072000; includeSubDomains; preload",
          },
          // ── Cache headers ──────────────────────────────────────────────
          {
            key: "Cache-Control",
            value: "public, max-age=3600, stale-while-revalidate=86400",
          },
          {
            key: "X-Device-Type",
            value: "desktop",
          },
        ],
      },
      {
        source: "/icons/:path*",
        headers: [
          { key: "Cache-Control", value: "public, max-age=2592000, immutable" },
        ],
      },
      {
        source: "/_next/static/:path*",
        headers: [
          { key: "Cache-Control", value: "public, max-age=31536000, immutable" },
        ],
      },
    ];
  },
};

module.exports = withNextIntl(withPWA(nextConfig));
