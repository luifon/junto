"""The icon files of the packages (ADR 0756): the program `junto-icon` that writes them from
the SVG source, and readers of PNG pictures, the macOS icon file, the Windows icon file and
the icon resource of a Windows program, for the checks of the packages."""
import os
import pathlib
import struct
import subprocess
import tempfile
import zlib

REPOSITORY = pathlib.Path(__file__).resolve().parent.parent
SOURCE = REPOSITORY / "packaging/icon/junto.svg"
# The program of a debug build, which the packaging tests and checks use.
TOOL = (pathlib.Path(os.environ.get("CARGO_TARGET_DIR", REPOSITORY / "target")) / "debug"
        / ("junto-icon.exe" if os.name == "nt" else "junto-icon"))
PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"


def run(tool, *arguments):
    """Runs the icon program; a failure reports the command and its error output."""
    tool = pathlib.Path(tool)
    if not tool.is_file():
        raise ValueError(f"the icon program {tool} does not exist: build it with cargo build -p junto-icon")
    command = [str(tool), *(str(argument) for argument in arguments)]
    result = subprocess.run(command, capture_output=True, text=True, timeout=300)
    if result.returncode:
        raise ValueError(f"{' '.join(command)} ended with code {result.returncode}: {result.stderr.strip()}")


def rendering(tool, size):
    """The PNG picture that the icon program writes for `size` pixels."""
    with tempfile.TemporaryDirectory(prefix="junto-icon-") as temporary:
        path = pathlib.Path(temporary) / "icon.png"
        run(tool, "png", size, path)
        return path.read_bytes()


class Picture:
    """A square picture with straight RGBA pixels, row by row from the top."""

    def __init__(self, size, rgba):
        if len(rgba) != size * size * 4:
            raise ValueError(f"a picture of {size} pixels has {size * size * 4} bytes, not {len(rgba)}")
        self.size, self.rgba = size, bytes(rgba)

    def __eq__(self, other):
        return isinstance(other, Picture) and (self.size, self.rgba) == (other.size, other.rgba)

    def __repr__(self):
        return f"Picture({self.size})"

    def pixel(self, x, y):
        start = (y * self.size + x) * 4
        return tuple(self.rgba[start:start + 4])

    def difference(self, other):
        """The largest difference of one channel of one pixel between two pictures."""
        if self.size != other.size:
            raise ValueError(f"a picture of {self.size} pixels is compared with one of {other.size}")
        return max((abs(a - b) for a, b in zip(self.rgba, other.rgba)), default=0)


def png(data):
    """The picture of a square PNG file of 8 bits per channel, without interlacing."""
    if data[:8] != PNG_SIGNATURE:
        raise ValueError("not a PNG picture")
    position, header, compressed = 8, None, bytearray()
    while position < len(data):
        length, kind = struct.unpack(">I4s", data[position:position + 8])
        body = data[position + 8:position + 8 + length]
        if len(body) != length:
            raise ValueError("a PNG chunk ends after the file")
        position += 12 + length
        if kind == b"IHDR":
            header = struct.unpack(">IIBBBBB", body)
        elif kind == b"IDAT":
            compressed += body
        elif kind == b"IEND":
            break
    if header is None:
        raise ValueError("the PNG picture has no header")
    width, height, depth, colour, _, _, interlace = header
    if width != height or depth != 8 or colour not in (2, 6) or interlace:
        raise ValueError(f"an unexpected PNG picture: {width}x{height}, depth {depth}, colour type {colour}, "
                         f"interlace {interlace}")
    channels = 4 if colour == 6 else 3
    raw = zlib.decompress(bytes(compressed))
    stride = width * channels
    if len(raw) != height * (stride + 1):
        raise ValueError("the PNG picture has the wrong amount of data")
    previous, rows = bytearray(stride), bytearray()
    for y in range(height):
        start = y * (stride + 1)
        method, row = raw[start], bytearray(raw[start + 1:start + 1 + stride])
        for i in range(stride):
            left = row[i - channels] if i >= channels else 0
            above = previous[i]
            corner = previous[i - channels] if i >= channels else 0
            if method == 1:
                row[i] = (row[i] + left) & 255
            elif method == 2:
                row[i] = (row[i] + above) & 255
            elif method == 3:
                row[i] = (row[i] + (left + above) // 2) & 255
            elif method == 4:
                estimate = left + above - corner
                distances = abs(estimate - left), abs(estimate - above), abs(estimate - corner)
                nearest = left if distances[0] <= distances[1] and distances[0] <= distances[2] else (
                    above if distances[1] <= distances[2] else corner)
                row[i] = (row[i] + nearest) & 255
            elif method != 0:
                raise ValueError(f"unknown PNG filter {method}")
        rows += row
        previous = row
    if channels == 3:
        rows = bytearray(b"".join(bytes(rows[i:i + 3]) + b"\xff" for i in range(0, len(rows), 3)))
    return Picture(width, rows)


def icns(data):
    """The entries of a macOS icon file, by type."""
    if data[:4] != b"icns" or struct.unpack(">I", data[4:8])[0] != len(data):
        raise ValueError("not a macOS icon file")
    entries, position = {}, 8
    while position < len(data):
        kind, length = struct.unpack(">4sI", data[position:position + 8])
        if length < 8 or position + length > len(data):
            raise ValueError(f"the entry {kind!r} ends after the file")
        entries[kind.decode("ascii")] = data[position + 8:position + length]
        position += length
    return entries


def run_length(code, count):
    """`count` values of the run-length code of the macOS icon file, and the bytes it used."""
    values, position = bytearray(), 0
    while len(values) < count:
        byte = code[position]
        if byte >= 128:
            values += bytes([code[position + 1]]) * (byte - 125)
            position += 2
        else:
            values += code[position + 1:position + 2 + byte]
            position += 2 + byte
    if len(values) != count:
        raise ValueError("a channel of the icon entry is longer than its picture")
    return bytes(values), position


def icns_picture(data, size):
    """The picture of one entry of a macOS icon file: PNG, or `ARGB` with coded channels."""
    if data[:8] == PNG_SIGNATURE:
        picture = png(data)
    elif data[:4] == b"ARGB":
        channels, position = [], 4
        for _ in range(4):
            values, used = run_length(data[position:], size * size)
            channels.append(values)
            position += used
        if position != len(data):
            raise ValueError("the ARGB entry has more data than its picture")
        alpha, red, green, blue = channels
        picture = Picture(size, bytes(value for pixel in zip(red, green, blue, alpha) for value in pixel))
    else:
        raise ValueError(f"an icon entry of an unknown form: {data[:4]!r}")
    if picture.size != size:
        raise ValueError(f"the entry of {size} pixels holds a picture of {picture.size}")
    return picture


def windows_picture(data, size):
    """One picture of a Windows icon: PNG, or a bitmap of 32 bits per pixel with its mask."""
    if data[:8] == PNG_SIGNATURE:
        picture = png(data)
        if picture.size != size:
            raise ValueError(f"the entry of {size} pixels holds a picture of {picture.size}")
        return picture
    header_size, width, height, planes, bits, compression = struct.unpack("<IiiHHI", data[:20])
    if (header_size, width, height, planes, bits, compression) != (40, size, 2 * size, 1, 32, 0):
        raise ValueError(f"an unexpected bitmap header for {size} pixels: "
                         f"{(header_size, width, height, planes, bits, compression)}")
    pixels = data[40:40 + size * size * 4]
    mask_row = (size + 31) // 32 * 4
    mask = data[40 + len(pixels):]
    if len(pixels) != size * size * 4 or len(mask) != mask_row * size:
        raise ValueError(f"the bitmap of {size} pixels has the wrong amount of data")
    rgba = bytearray()
    for y in reversed(range(size)):
        for x in range(size):
            blue, green, red, alpha = pixels[(y * size + x) * 4:(y * size + x) * 4 + 4]
            transparent = bool(mask[y * mask_row + x // 8] & (0x80 >> (x % 8)))
            if transparent != (alpha == 0):
                raise ValueError(f"the mask of the bitmap of {size} pixels differs from its alpha at {x}, {y}")
            rgba += bytes([red, green, blue, alpha])
    return Picture(size, rgba)


def _icon_directory(data, entry_size):
    reserved, kind, count = struct.unpack("<HHH", data[:6])
    if reserved or kind != 1:
        raise ValueError("not an icon directory")
    for index in range(count):
        entry = data[6 + entry_size * index:6 + entry_size * (index + 1)]
        width, height, colours, _, planes, bits, length = struct.unpack("<BBBBHHI", entry[:12])
        if width != height or colours or (planes, bits) != (1, 32):
            raise ValueError(f"an unexpected icon entry: {(width, height, colours, planes, bits)}")
        yield width or 256, length, entry[12:]


def ico(data):
    """The pictures of a Windows icon file, by side."""
    pictures = {}
    for size, length, rest in _icon_directory(data, 16):
        offset = struct.unpack("<I", rest)[0]
        pictures[size] = windows_picture(data[offset:offset + length], size)
    return pictures


RT_ICON, RT_GROUP_ICON = 3, 14


def program_icon(data, group=1):
    """The pictures of the icon group number `group` of a Windows program or library, by side."""
    if data[:2] != b"MZ":
        raise ValueError("not a Windows program")
    header = struct.unpack("<I", data[0x3c:0x40])[0]
    if data[header:header + 4] != b"PE\0\0":
        raise ValueError("not a Windows program")
    _, sections, _, _, _, optional_size, _ = struct.unpack("<HHIIIHH", data[header + 4:header + 24])
    optional = header + 24
    magic = struct.unpack("<H", data[optional:optional + 2])[0]
    directories = optional + (112 if magic == 0x20b else 96)
    resource_address, _ = struct.unpack("<II", data[directories + 16:directories + 24])
    table = optional + optional_size

    def position(address):
        for index in range(sections):
            start = table + 40 * index
            virtual_size, virtual_address, raw_size, raw_position = struct.unpack("<IIII", data[start + 8:start + 24])
            if virtual_address <= address < virtual_address + max(virtual_size, raw_size):
                return raw_position + address - virtual_address
        raise ValueError(f"no section holds the address {address:#x}")

    if not resource_address:
        raise ValueError("the program has no resources")
    base = position(resource_address)

    def directory(offset):
        named, numbered = struct.unpack("<HH", data[base + offset + 12:base + offset + 16])
        entries = {}
        for index in range(named + numbered):
            start = base + offset + 16 + 8 * index
            name, target = struct.unpack("<II", data[start:start + 8])
            if not name & 0x80000000:
                entries[name] = target
        return entries

    def resource(kind, number):
        try:
            names = directory(directory(0)[kind] & 0x7fffffff)
            languages = directory(names[number] & 0x7fffffff)
        except KeyError:
            raise ValueError(f"the program has no resource of type {kind} and number {number}") from None
        if len(languages) != 1:
            raise ValueError(f"the resource of type {kind} and number {number} has {len(languages)} languages")
        entry = base + next(iter(languages.values()))
        address, size = struct.unpack("<II", data[entry:entry + 8])
        start = position(address)
        return data[start:start + size]

    pictures = {}
    for size, length, rest in _icon_directory(resource(RT_GROUP_ICON, group), 14):
        image = resource(RT_ICON, struct.unpack("<H", rest)[0])
        if len(image) != length:
            raise ValueError(f"the icon of {size} pixels has {len(image)} bytes, the group states {length}")
        pictures[size] = windows_picture(image, size)
    return pictures
