-- LinuxAI's screen. Hyprland draws it; the AI drives it through the compositor organ (linuxai-core compositor).
-- Not a desktop: one place where the work (the session, pages, pictures) appears and the AI arranges it.
-- Hyprland 0.55+ configuration is Lua. The start-up comes first so a mistake further down can't stop LinuxAI.

hl.on("hyprland.start", function ()
    -- Bring the session environment to the user services; the shared browser restarts on screen, not headless.
    hl.exec_cmd("systemctl --user import-environment WAYLAND_DISPLAY HYPRLAND_INSTANCE_SIGNATURE XDG_CURRENT_DESKTOP && systemctl --user restart linuxai-browser")
    hl.exec_cmd("/usr/local/bin/linuxai-core compositor")
    hl.exec_cmd("mkdir -p $HOME/.local/state/linuxai && quickshell -p /etc/linuxai/quickshell/shell.qml > $HOME/.local/state/linuxai/quickshell.log 2>&1")
    hl.exec_cmd("foot --title LinuxAI /usr/local/bin/linuxai-session")
end)

-- Each group on its own, so one unknown option only loses that group.
local function set(name, f) local ok, e = pcall(f); if not ok then print("linuxai: " .. name .. ": " .. tostring(e)) end end

set("monitor", function () hl.monitor({ output = "", mode = "preferred", position = "auto", scale = "auto" }) end)
set("env", function () hl.env("XDG_CURRENT_DESKTOP", "Hyprland"); hl.env("XCURSOR_SIZE", "24") end)
set("general", function () hl.config({ general = { gaps_in = 6, gaps_out = 10, border_size = 2, layout = "master",
    col = { active_border = "rgb(46c2c9)", inactive_border = "rgb(233140)" } } }) end)
-- The Workspace is the main window (the compositor organ puts it there): 70% of the screen, the session beside it.
set("master", function () hl.config({ master = { mfact = 0.7 } }) end)
set("decoration", function () hl.config({ decoration = { rounding = 8, blur = { enabled = true } } }) end)
set("animations", function () hl.config({ animations = { enabled = true } }) end)
set("misc", function () hl.config({ misc = { disable_hyprland_logo = true, force_default_wallpaper = 0 } }) end)
set("input", function () hl.config({ input = { kb_layout = "us" } }) end)

-- A few keys while there's no voice or Workspace screen yet.
local mod = "SUPER"
set("binds", function ()
    hl.bind(mod .. " + Return", hl.dsp.exec_cmd("foot /usr/local/bin/linuxai-session"))
    hl.bind(mod .. " + Q", hl.dsp.window.close())
    hl.bind(mod .. " + F", hl.dsp.window.fullscreen({ action = "toggle" }))
    for _, d in ipairs({ "left", "right", "up", "down" }) do
        hl.bind(mod .. " + " .. d, hl.dsp.focus({ direction = d }))
    end
end)
