# Disclaimer

ThermalAI (AIThermal-Rust) is an independent, community-developed project and is
**not affiliated with, endorsed by, or supported by Xiaomi, Qualcomm, Google, or
any device manufacturer.**

## No Warranty

This software is provided "AS IS", without warranty of any kind, express or
implied, including but not limited to the warranties of merchantability, fitness
for a particular purpose, and non-infringement. In no event shall the author(s)
or contributors be liable for any claim, damages, or other liability, whether in
an action of contract, tort, or otherwise, arising from, out of, or in connection
with the software or the use or other dealings in the software.

## Use At Your Own Risk

This module modifies low-level system behavior, including CPU governors, GPU
power states, charging current limits, thermal management interfaces, and kernel
scheduling parameters, by directly writing to sysfs nodes as the root user. This
is inherently more invasive than a typical Android app. By installing and using
this module, you acknowledge and accept that:

- Incorrect or unexpected behavior could potentially affect device stability,
  battery health/longevity, thermal safety, or performance.
- You are solely responsible for any consequences of using this software,
  including but not limited to bootloops, battery degradation, device damage, or
  voided manufacturer warranties resulting from root access and/or custom ROM
  usage in general (a prerequisite for using this module at all).
- You should maintain your own backups and be prepared to restore your device via
  recovery/fastboot if something goes wrong.
- The author(s) provide this software in the hope that it is useful, but make no
  guarantee of fitness for any particular device, ROM, or use case.

## Device Compatibility Scope

**This module was built and tested exclusively on a POCO F6 (codename: peridot,
Snapdragon 8s Gen 3 / SM8635) running Xiaomi HyperOS 3.** Every tuning decision,
sysfs node path, hardware capability check, and thermal threshold in this project
was verified against real, logged behavior from this specific device and ROM
combination.

This module **may or may not work correctly on other devices**, including other
Snapdragon devices, other Xiaomi/HyperOS devices, or the same device on a
different ROM (custom AOSP-based ROMs, other MIUI/HyperOS versions, etc.). While
the hardware-discovery-first architecture is designed to fail safely (skipping
unavailable/unwritable capabilities rather than assuming they exist), this has
only been directly verified on the device and ROM described above. If you use
this on a different device:

- Review `thermalai_state/hardware_report.txt` after installation to see exactly
  what was and wasn't detected as available on your device.
- Consider starting with `disable_tweaks = true` in `profiles.conf` to verify the
  module runs stably before enabling active tuning.
- Report any issues, but understand that support and further tuning for devices
  other than the POCO F6 (peridot) may be limited, since the author's ability to
  verify behavior is limited to the device this project was built for.

## Experimental Features

Some features in this project are explicitly experimental and off-by-default or
newly-added (check `profiles.conf` for the current list, e.g.
`adaptive_governor_enabled`). These have undergone less real-world testing than
the core thermal/charging/gaming-detection functionality and should be enabled
with the above understanding in mind.