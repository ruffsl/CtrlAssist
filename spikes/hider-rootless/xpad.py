# A wired Xbox 360 pad on the USB bus, for the xpad driver: a USB gadget
# made through configfs and FunctionFS, plugged into the dummy host
# controller. It enumerates and holds; it sends no input.
#
#   xpad.py <serial>
import os
import struct
import subprocess
import sys
import time

GADGET = "/sys/kernel/config/usb_gadget/xpad"
FFS = "/dev/ffs-xpad"


def write(path, value):
    with open(path, "w") as file:
        file.write(value)


def descriptors():
    interface = struct.pack(
        "<BBBBBBBBB",
        9,  # bLength
        4,  # INTERFACE
        0,  # bInterfaceNumber
        0,  # bAlternateSetting
        2,  # bNumEndpoints
        0xFF,  # vendor-specific: what xpad matches an Xbox 360 pad by
        0x5D,
        0x01,
        1,  # iInterface
    )

    def endpoint(address, interval):
        # Interrupt, 32 bytes.
        return struct.pack("<BBBBHB", 7, 5, address, 3, 32, interval)

    full = interface + endpoint(0x81, 4) + endpoint(0x02, 8)
    high = interface + endpoint(0x81, 4) + endpoint(0x02, 4)
    body = struct.pack("<III", 1 | 2, 3, 3) + full + high
    # FUNCTIONFS_DESCRIPTORS_MAGIC_V2, then the length of the whole.
    return struct.pack("<II", 3, 8 + len(body)) + body


def strings():
    text = b"xpad\0"
    body = struct.pack("<IIH", 1, 1, 0x0409) + text
    return struct.pack("<II", 2, 8 + len(body)) + body


def main(serial):
    for module in ["dummy_hcd", "libcomposite", "usb_f_fs"]:
        subprocess.run(["modprobe", module], check=True)
    os.makedirs(GADGET, exist_ok=True)
    write(f"{GADGET}/idVendor", "0x045e")
    write(f"{GADGET}/idProduct", "0x028e")
    write(f"{GADGET}/bcdDevice", "0x0114")
    write(f"{GADGET}/bcdUSB", "0x0200")
    write(f"{GADGET}/bDeviceClass", "0xff")
    write(f"{GADGET}/bDeviceSubClass", "0xff")
    write(f"{GADGET}/bDeviceProtocol", "0xff")
    os.makedirs(f"{GADGET}/strings/0x409", exist_ok=True)
    write(f"{GADGET}/strings/0x409/serialnumber", serial)
    write(f"{GADGET}/strings/0x409/manufacturer", "Spike")
    write(f"{GADGET}/strings/0x409/product", "Controller")
    os.makedirs(f"{GADGET}/configs/c.1/strings/0x409", exist_ok=True)
    write(f"{GADGET}/configs/c.1/strings/0x409/configuration", "xpad")
    os.makedirs(f"{GADGET}/functions/ffs.xpad", exist_ok=True)
    if not os.path.exists(f"{GADGET}/configs/c.1/ffs.xpad"):
        os.symlink(f"{GADGET}/functions/ffs.xpad", f"{GADGET}/configs/c.1/ffs.xpad")
    os.makedirs(FFS, exist_ok=True)
    subprocess.run(["mount", "-t", "functionfs", "xpad", FFS], check=True)

    ep0 = os.open(f"{FFS}/ep0", os.O_RDWR)
    os.write(ep0, descriptors())
    os.write(ep0, strings())
    udc = sorted(os.listdir("/sys/class/udc"))[0]
    write(f"{GADGET}/UDC", udc)
    print(f"bound to {udc}", flush=True)

    # Answer the host: an event is twelve bytes, a setup request then its
    # type. A request this function doesn't know is answered empty.
    while True:
        event = os.read(ep0, 12)
        request_type, request, value, index, length, kind = struct.unpack("<BBHHHB3x", event)
        names = {0: "bind", 1: "unbind", 2: "enable", 3: "disable", 4: "setup", 5: "suspend", 6: "resume"}
        print(f"event {names.get(kind, kind)}", flush=True)
        if kind == 4:
            if request_type & 0x80:
                os.write(ep0, b"")
            else:
                os.read(ep0, length)


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "XPADSPIKE1")
    time.sleep(1)
