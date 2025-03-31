import { defineConfig } from "vitepress"

// https://vitepress.dev/reference/site-config
export default defineConfig({
  title: "emissary",
  description: "A VitePress Site",
  base: '/emissary/',
  themeConfig: {
    // https://vitepress.dev/reference/default-theme-config
    nav: [
      { text: "Home", link: "/" },
      { text: "Contact", link: "/contact" }
    ],

    sidebar: [
      {
        text: "User guide",
        items: [
          { text: "Quick start", link: "/quick-start.md" },
          { text: "Router configuration", link: "/router-configuration.md" },
          { text: "Eepsites", link: "/eepsite.md" },
          { text: "Torrents", link: "/torrents.md" },
          { text: "IRC and email", link: "/irc-email.md" },
        ]
      },
    ],

    socialLinks: [
      { icon: "github", link: "https://github.com/altonen/emissary" }
    ],
  }
})
