import type { Metadata, Viewport } from "next";
import { notFound } from "next/navigation";
import { NextIntlClientProvider } from "next-intl";
import { getMessages, getFormatter } from "next-intl/server";
import { routing, Locale } from "@/i18n/routing";
import { isRtlLocale, getDirection } from "@/i18n/rtl";
import "./globals.css";
import { ThemeProvider } from "@/components/providers/ThemeProvider";
import { QueryProvider } from "@/components/providers/QueryProvider";
import { AccessibilityProvider } from "@/components/providers/AccessibilityProvider";
import { ErrorProvider } from "@/components/providers/ErrorProvider";
import { CollaborationProvider } from "@/components/providers/CollaborationProvider";
import { ErrorBoundary } from "@/components/ui/ErrorBoundary";
import { AppLayout } from "@/components/layout/AppLayout";
import { AuthProvider } from "@/hooks/useAuth";
import { TxStatusProvider } from "@/hooks/useTxStatus";
import { WalletProvider } from "@/hooks/useWallet";
import { NotificationProvider } from "@/contexts/NotificationContext";
import { WebVitalsInit } from "@/components/providers/WebVitalsInit";
import { DragAndDropProvider } from "@/components/providers/DragAndDropProvider";
import { RumProvider } from "@/components/providers/RumProvider";
import { RouteChangeMonitor } from "@/components/navigation/RouteChangeMonitor";
import { ConsentBanner } from "@/components/providers/ConsentBanner";


export function generateStaticParams() {
  return routing.locales.map((locale) => ({ locale }));
}

export const metadata: Metadata = {
  title: "ArenaX",
  description: "Competitive Gaming Platform",
  manifest: "/manifest.json",
  appleWebApp: {
    capable: true,
    statusBarStyle: "default",
    title: "ArenaX",
  },
  other: {
    "mobile-web-app-capable": "yes",
  },
};

// WCAG 2.1 SC 1.4.4 (Resize Text, AA): never disable pinch-to-zoom.
// iOS auto-zoom on input focus is prevented by font-size: 16px in globals.css
// (see the `input, textarea, select { font-size: 16px }` rule under @supports).
export const viewport: Viewport = {
  themeColor: "#111827",
  width: "device-width",
  initialScale: 1,
};

export default async function RootLayout({
  children,
  params,
}: {
  children: React.ReactNode;
  params: { locale: Locale };
}) {
  const { locale } = params;

  if (!routing.locales.includes(locale)) {
    notFound();
  }

  const messages = await getMessages();
  const direction = getDirection(locale);
  const isRtl = isRtlLocale(locale);
  const format = await getFormatter();

  return (
    <html
      lang={locale}
      dir={direction}
      data-direction={direction}
      suppressHydrationWarning
    >
      <head>
        <link rel="apple-touch-icon" href="/icons/icon-192x192.png" />
        <link rel="icon" type="image/png" sizes="192x192" href="/icons/icon-192x192.png" />
        <link rel="icon" type="image/png" sizes="512x512" href="/icons/icon-512x512.png" />
      </head>
      <body
        className="font-sans antialiased"
        // Logical direction is inherited from <html dir>; numeric and sign
        // order stay LTR via the `bdi`/`tabular-nums` utilities in globals.css.
        data-rtl={isRtl ? "true" : "false"}
      >
        <NextIntlClientProvider
          locale={locale}
          messages={messages}
          formats={{
            number: {
              money: {
                style: "currency",
                currency: "USD",
                currencyDisplay: "narrowSymbol",
              },
              percent: { style: "percent", maximumFractionDigits: 1 },
              compact: { notation: "compact", maximumFractionDigits: 1 },
            },
            dateTime: {
              short: { dateStyle: "short" },
              long: { dateStyle: "long", timeStyle: "short" },
            },
            list: {},
          }}
          timeZone="UTC"
          now={new Date()}
        >
          <ThemeProvider
            attribute="class"
            defaultTheme="system"
            enableSystem
            disableTransitionOnChange
            storageKey="theme"
          >
            <ErrorProvider>
              <CollaborationProvider>
                <ErrorBoundary>
                  <AccessibilityProvider>
                    <QueryProvider>
                      <AuthProvider>
                        <WalletProvider>
                          <TxStatusProvider>
                            <NotificationProvider>
                              <RumProvider>
                                <DragAndDropProvider>
                                  <RouteChangeMonitor />
                                  <AppLayout direction={direction}>
                                    {children}
                                  </AppLayout>
                                </DragAndDropProvider>
                              </RumProvider>
                              <ConsentBanner />
                            </NotificationProvider>
                          </TxStatusProvider>
                        </WalletProvider>
                      </AuthProvider>
                    </QueryProvider>
                  </AccessibilityProvider>
                </ErrorBoundary>
              </CollaborationProvider>
            </ErrorProvider>
          </ThemeProvider>
        </NextIntlClientProvider>
      </body>
    </html>
  );
}

// Expose the active direction to client components without prop drilling.
export function generateMetadata(): Metadata {
  return metadata;
}

// Keep the formatter helper referenced so tree-shaking never drops the
// locale-aware number/date formatting used by server components.
void getFormatter;
