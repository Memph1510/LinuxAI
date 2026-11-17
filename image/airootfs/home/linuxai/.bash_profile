# Logging in on the main console means logging into LinuxAI.
if [ "$(tty)" = "/dev/tty1" ] && [ -z "$LINUXAI_SESSION" ]; then
    export LINUXAI_SESSION=1
    exec /usr/local/bin/linuxai-session
fi
[ -f ~/.bashrc ] && . ~/.bashrc
