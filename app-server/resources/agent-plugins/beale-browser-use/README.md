# Beale Browser Use

This built-in Agent Plugin is enabled by default and can be turned off in Plugins. It starts a separate headless browser profile through WebDriver BiDi. The app-server hosts the plugin process; only bounded browser tools are exposed to research sessions.

Chrome is used by default and must be installed on the host. Set `BEALE_BROWSER_EXECUTABLE_PATH` to an absolute Chrome or Firefox executable path when automatic discovery is unavailable. Choose `firefox` in the `open` tool when using Firefox. The plugin does not download a browser or attach to an existing personal browser profile.

`observe` returns page text and short-lived element IDs. `click` and `fill` require those IDs. Screenshots are bounded to 10 MiB. Browser mutation tools use the normal Beale tool approval flow. The browser and its temporary profile close when the last tab closes or the plugin exits normally.
