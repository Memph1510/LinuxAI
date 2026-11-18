# LinuxAI install image

This folder builds the LinuxAI live ISO: an Arch-based image that boots straight into the LinuxAI session.
There is no desktop. The first screen is the AI.

## What's on it (v0.1)

| Piece | What it does |
|---|---|
| `linuxai-core` (system service, starts at boot) | Senses the machine (power, battery, CPU/memory/I/O pressure, temperature, failed services, scheduler state) and controls it through the kernel's steering surfaces: power profiles, the sched_ext CPU scheduler policy, cgroup resource shares, services, sysctl. Runs plans in the LinuxAI action language and its built-in rules. Keeps its memory in `/var/lib/linuxai`. |
| LinuxAI session (tty1, automatic login) | Where you land. You talk in English. You see what the system is doing as it happens, and you answer its questions. |
| `linuxai-mail` (system service, once `/etc/linuxai/mail.env` exists) | Email in, claims out, never instructions (Rust; see docs/MAIL.md) |
| `linuxai-reach` (user service, Rust) | Reaches you when the AI is undecided or a follow-up is due. Set `LINUXAI_NOTIFY_CMD` in `~/.config/linuxai/env` to any command that sends a message from stdin (an SMS gateway, a push service), e.g. `LINUXAI_NOTIFY_CMD=curl -s -d @- https://ntfy.sh/<your-topic>`. |
| `linuxai-mind` (user service) | The AI working behind the scenes. It handles anything the rules hand it ("a service failed, figure it out"). It starts once an API key is set. |
| Linux kernel + drivers, `scx-scheds`/`scx-tools`, `power-profiles-daemon` | The hardware layer and the kernel controls the core uses |

On first start the session asks for an Anthropic API key, which is stored only on the machine, in
`~/.config/linuxai/env`. Without a key, the machine still runs on its rules. It just can't think about new
situations.

## Getting the ISO

- **From GitHub:** every push builds it. Open the repo's **Actions** tab, pick the latest **build** run, and
  download the `linuxai-iso` artifact (a zip containing the `.iso`).
- **Build it yourself:** `./image/build.sh` (Docker, or directly on Arch). The ISO lands in `out/`.

## Trying it on Windows in a VM (recommended first step)

Use VirtualBox (free) or Hyper-V (built into Windows Pro):

**VirtualBox**
1. New → Name `LinuxAI`, Type `Linux`, Version `Arch Linux (64-bit)`.
2. Memory 4096 MB or more, 2+ CPUs.
3. Skip the hard disk for now (live mode runs from the ISO).
4. Settings → Storage → choose the LinuxAI `.iso` for the optical drive.
5. Start. Pick **LinuxAI live** in the boot menu (it's the default).

**Hyper-V**
1. New → Virtual Machine → Generation 2.
2. Memory 4096 MB, network "Default Switch", no virtual disk.
3. Settings → Security → **turn off Secure Boot** (or use the "Microsoft UEFI Certificate Authority"
   template), then attach the ISO under SCSI controller → DVD drive.
4. Start.

### Live mode is temporary

The live ISO runs from memory. Anything it learns is lost when the VM powers off. Installing to a disk (and later
dual boot) is the next milestone.

## Automated boot test

```bash
python3 image/test-boot.py out/linuxai-*.iso --shots out/
```

This boots the ISO headless in QEMU, logs in on the serial console, and checks:

- the core is running and sensing;
- its rules are loaded;
- a plan runs;
- a simulated unplug really switches the power profile;
- you're logged into the LinuxAI session.

It also saves a screenshot of the main screen.
