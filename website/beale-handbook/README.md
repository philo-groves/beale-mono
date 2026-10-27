# Beale Handbook

The `Beale Handbook` WordPress plugin serves `/docs/`, seven section indexes, and 29 guides from `content.json`. It replaces the legacy Docs page template on these routes while active. The site navigation continues to point to `/docs/`.

Edit guide text and links in `content.json`, then run:

```sh
node website/beale-handbook/validate.mjs
```

The validator checks the requested guide inventory, internal links, and the iOS availability statement. To deploy, zip the `website/beale-handbook` directory, upload the archive through WordPress Plugins → Add Plugin → Upload Plugin, activate it, and verify `/docs/` and an article. For an update, use WordPress's replace-current plugin flow. Change the plugin and stylesheet version when changing PHP or CSS so caches refresh.
