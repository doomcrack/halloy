import footnote from "markdown-it-footnote";
import { defineConfig } from "vitepress";

const releaseLabel = process.env.DOCS_VERSION ?? "Latest Release";
const releaseLink =
  process.env.DOCS_CHANNEL === "nightly" ? "https://halloy.chat" : "/";
const nightlyLabel = process.env.DOCS_SHA
  ? `Nightly (${process.env.DOCS_SHA.slice(0, 7)})`
  : "Nightly";
const nightlyLink =
  process.env.DOCS_CHANNEL === "nightly" ? "/" : "https://nightly.halloy.chat";
const docsChannel =
  process.env.DOCS_CHANNEL === "nightly" ? nightlyLabel : releaseLabel;

const guidesItems = [
  { text: "Custom Themes", link: "/guides/custom-themes" },
  { text: "Internal Buffers", link: "/guides/internal-buffers" },
  { text: "Optional Features", link: "/guides/optional-features" },
  { text: "Portable Mode", link: "/guides/portable-mode" },
  { text: "Single Pane", link: "/guides/single-pane" },
  { text: "Unix Signals", link: "/guides/unix-signals" },
  { text: "URL Schemes", link: "/guides/url-schemes" },
];

const configurationItems = [
  { text: "Actions", link: "/configuration/actions" },
  { text: "Buffer", link: "/configuration/buffer" },
  {
    text: "Check for Update on Launch",
    link: "/configuration/check-for-update-on-launch",
  },
  { text: "Commands", link: "/commands" },
  { text: "Context Menu", link: "/configuration/context-menu" },
  { text: "Display", link: "/configuration/display" },
  { text: "Font", link: "/configuration/font" },
  { text: "Keyboard", link: "/configuration/keyboard" },
  { text: "Logos", link: "/configuration/logos" },
  { text: "Logs", link: "/configuration/logs" },
  { text: "Modules", link: "/configuration/modules" },
  { text: "Notifications", link: "/configuration/notifications" },
  { text: "Pane", link: "/configuration/pane" },
  { text: "Platform Specific", link: "/configuration/platform-specific" },
  { text: "Preview", link: "/configuration/preview" },
  { text: "Runtime", link: "/configuration/runtime" },
  { text: "Scale factor", link: "/configuration/scale-factor" },
  { text: "Sidebar", link: "/configuration/sidebar" },
  { text: "Themes", link: "/configuration/themes" },
  { text: "Tooltips", link: "/configuration/tooltips" },
  { text: "Window", link: "/configuration/window" },
];

export default defineConfig({
  title: "Frigicom",
  description:
    "Frigicom is a standalone desktop chat client for the Logos network, written in Rust with the iced GUI library. It is a fork of halloy.",
  base: process.env.DOCS_BASE ?? "/",
  appearance: "force-dark",
  cleanUrls: true,
  head: [["link", { rel: "icon", type: "image/png", href: "/favicon.png" }]],
  markdown: {
    config: (md) => {
      md.use(footnote);
    },
    container: {
      tipLabel: "💡 Tip",
      warningLabel: "⚠️ Warning",
      dangerLabel: "⚠️ Danger",
      infoLabel: "💡 Info",
      detailsLabel: "Details",
    },
  },
  themeConfig: {
    logo: "/logo.png",
    siteTitle: `Frigicom <span class="VPBadge info mobile-only">${docsChannel}</span>`,
    search: {
      provider: "local",
    },
    outline: {
      level: [2, 4],
    },
    nav: [
      {
        text: docsChannel,
        items: [
          { text: releaseLabel, link: releaseLink, target: "_self" },
          { text: nightlyLabel, link: nightlyLink, target: "_self" },
        ],
      },
      {
        text: "Themes",
        link: "https://themes.halloy.chat/",
      },
    ],
    editLink: {
      pattern: "https://github.com/doomcrack/halloy/edit/main/docs/:path",
    },
    socialLinks: [
      { icon: "github", link: "https://github.com/doomcrack/halloy" },
    ],
    sidebar: {
      "/configuration": [
        {
          items: [
            { text: "Installation", link: "/installation" },
            { text: "Getting Started", link: "/getting-started" },
            {
              text: "Configuration",
              link: "/configuration",
              collapsed: false,
              items: configurationItems,
            },
            {
              text: "Guides",
              collapsed: false,
              items: guidesItems,
            },
            { text: "Contributing", link: "/contributing" },
            { text: "Testing", link: "/testing" },
            { text: "Logos modules", link: "/logos-modules" },
          ],
        },
      ],
      "/": [
        {
          items: [
            { text: "Installation", link: "/installation" },
            { text: "Getting Started", link: "/getting-started" },
            {
              text: "Configuration",
              link: "/configuration",
              collapsed: true,
              items: configurationItems,
            },
            {
              text: "Guides",
              collapsed: false,
              items: guidesItems,
            },
            { text: "Contributing", link: "/contributing" },
            { text: "Testing", link: "/testing" },
            { text: "Logos modules", link: "/logos-modules" },
          ],
        },
      ],
    },
  },
});
