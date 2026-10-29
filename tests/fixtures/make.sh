#!/bin/sh
# Rebuilds the files in this folder from QR codes drawn by zint: fax TIFFs, a scanned PDF with
# one page per image codec, and an AES-256 encrypted PDF. The tools only run here; none ships.
# Needs (Debian or Ubuntu): zint libtiff-tools img2pdf jbig2 libopenjp2-tools qpdf
# libjpeg-turbo-progs python3
set -eu
out=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cd "$tmp"

# draw NAME TEXT: NAME.pbm and NAME.pgm, 240 x 240 pixels with a QR of TEXT in the middle at
# 6 pixels per module; blank when TEXT is empty.
draw() {
    if [ -n "$2" ]; then zint -b QRCODE -d "$2" --dump >"$1.hex"; else : >"$1.hex"; fi
    python3 - "$1" <<'EOF'
import sys
name = sys.argv[1]
rows = [bytes.fromhex(line) for line in open(name + ".hex") if line.strip()]
size, scale = 240, 6
left = (size - len(rows) * scale) // 2

def dark(x, y):
    mx, my = (x - left) // scale, (y - left) // scale
    return x >= left and y >= left and mx < len(rows) and my < len(rows) and rows[my][mx // 8] >> (7 - mx % 8) & 1

pixels = [[dark(x, y) for x in range(size)] for y in range(size)]
grey = bytes(0 if d else 255 for row in pixels for d in row)
open(name + ".pgm", "wb").write(b"P5\n%d %d\n255\n" % (size, size) + grey)
bits = bytes(sum(row[x + i] << (7 - i) for i in range(8)) for row in pixels for x in range(0, size, 8))
open(name + ".pbm", "wb").write(b"P4\n%d %d\n" % (size, size) + bits)
EOF
}

# A 3-page Group 4 fax (min-is-white) with a code on page 2 only.
draw blank ""
draw two "fax page 2"
ppm2tiff -c g4 -r 240 blank.pbm blank.tif
ppm2tiff -c g4 -r 240 two.pbm two.tif
tiffcp blank.tif two.tif blank.tif "$out/fax.tif"

# A Group 3 page, which isn't supported, before a Group 4 one.
draw g3 "group 3"
draw g4 "group 4"
ppm2tiff -c g3 -r 240 g3.pbm g3.tif
ppm2tiff -c g4 -r 240 g4.pbm g4.tif
tiffcp g3.tif g4.tif "$out/fax-g3.tif"

# One scanned page per image codec, each QR holding its filter's name.
for codec in DCTDecode FlateDecode CCITTFaxDecode JBIG2Decode JPXDecode; do draw "$codec" "$codec"; done
cjpeg -grayscale -quality 90 DCTDecode.pgm >dct.jpg
img2pdf dct.jpg -o 1.pdf
img2pdf FlateDecode.pgm -o 2.pdf
ppm2tiff -c g4 -r 240 CCITTFaxDecode.pbm ccitt.tif
img2pdf ccitt.tif -o 3.pdf
jbig2 -p JBIG2Decode.pbm >page.jb2
python3 - <<'EOF'
data = open("page.jb2", "rb").read()
content = b"q 180 0 0 180 0 0 cm /Im0 Do Q"
image = b"<< /Type /XObject /Subtype /Image /Width 240 /Height 240 /ColorSpace /DeviceGray"
image += b" /BitsPerComponent 1 /Filter /JBIG2Decode /Length %d >>\nstream\n" % len(data)
objects = [
    b"<< /Type /Catalog /Pages 2 0 R >>",
    b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 180 180]"
    b" /Resources << /XObject << /Im0 4 0 R >> >> /Contents 5 0 R >>",
    image + data + b"\nendstream",
    b"<< /Length %d >>\nstream\n" % len(content) + content + b"\nendstream",
]
pdf, offsets = b"%PDF-1.5\n", []
for i, body in enumerate(objects):
    offsets.append(len(pdf))
    pdf += b"%d 0 obj\n" % (i + 1) + body + b"\nendobj\n"
xref = len(pdf)
pdf += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objects) + 1)
pdf += b"".join(b"%010d 00000 n \n" % o for o in offsets)
pdf += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objects) + 1, xref)
open("4.pdf", "wb").write(pdf)
EOF
opj_compress -i JPXDecode.pgm -o jpx.jp2 >/dev/null
img2pdf jpx.jp2 -o 5.pdf
qpdf --empty --pages 1.pdf 2.pdf 3.pdf 4.pdf 5.pdf -- "$out/scan.pdf"

# AES-256 with the user password "open-sesame".
draw secret "secret page"
img2pdf secret.pgm -o secret.pdf
qpdf --encrypt --user-password=open-sesame --owner-password=owner --bits=256 -- secret.pdf "$out/aes.pdf"

ls -l "$out"
