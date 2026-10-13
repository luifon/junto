"""Names used by release archives on supported systems."""
import platform


def architecture(machine=None):
    machine = (platform.machine() if machine is None else machine).lower()
    names = {"arm64": "aarch64", "aarch64": "aarch64", "x86_64": "x86_64", "amd64": "x86_64"}
    if machine not in names:
        raise ValueError(f"unsupported release architecture: {machine}")
    return names[machine]
