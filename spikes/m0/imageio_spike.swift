// M0 spike: measure what macOS ImageIO can do for Sony ARW + HIF.
// Build: swiftc -O spikes/m0/imageio_spike.swift -o <out>/imageio_spike
// Usage:
//   imageio_spike decode <file>...            full-res decode timing + bit depth
//   imageio_spike thumbs <maxPx> <file>...    embedded-preview thumbnail timing
//   imageio_spike encode <src.HIF> <out.HIF>  decode at 16-bit, re-encode HEIC, keep metadata
//   imageio_spike pair <shootDir>             pair ARW/HIF/JPG across a folder + format subfolders

import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

func now() -> Double { CFAbsoluteTimeGetCurrent() }
func ms(_ t: Double) -> String { String(format: "%.0f ms", (now() - t) * 1000) }

func source(_ path: String) -> CGImageSource? {
    CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, [kCGImageSourceShouldCache: false] as CFDictionary)
}

func props(_ src: CGImageSource) -> [String: Any] {
    (CGImageSourceCopyPropertiesAtIndex(src, 0, nil) as? [String: Any]) ?? [:]
}

func captureDate(_ p: [String: Any]) -> String? {
    let exif = p[kCGImagePropertyExifDictionary as String] as? [String: Any]
    let date = exif?[kCGImagePropertyExifDateTimeOriginal as String] as? String
    let sub = exif?[kCGImagePropertyExifSubsecTimeOriginal as String] as? String
    return date.map { sub != nil ? "\($0).\(sub!)" : $0 }
}

func decode(_ paths: [String]) {
    for path in paths {
        let t = now()
        guard let src = source(path), let img = CGImageSourceCreateImageAtIndex(src, 0, nil) else {
            print("FAIL \(path)"); continue
        }
        // Force the actual decode by drawing into a 16-bit RGBA buffer (what the editor would need).
        let w = img.width, h = img.height
        let cs = CGColorSpace(name: CGColorSpace.displayP3)!
        let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 16, bytesPerRow: 0, space: cs,
                            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue | CGBitmapInfo.byteOrder16Little.rawValue)!
        ctx.draw(img, in: CGRect(x: 0, y: 0, width: w, height: h))
        let p = props(src)
        print("\((path as NSString).lastPathComponent): \(w)x\(h) depth=\(p[kCGImagePropertyDepth as String] ?? "?") bpc=\(img.bitsPerComponent) decode+draw16=\(ms(t))")
    }
}

func thumbs(_ maxPx: Int, _ paths: [String]) {
    let total = now()
    for path in paths {
        let t = now()
        guard let src = source(path) else { print("FAIL \(path)"); continue }
        let opts: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageIfAbsent: true, // use the embedded preview when there is one
            kCGImageSourceThumbnailMaxPixelSize: maxPx,
            kCGImageSourceCreateThumbnailWithTransform: true,
        ]
        guard let th = CGImageSourceCreateThumbnailAtIndex(src, 0, opts as CFDictionary) else { print("FAIL thumb \(path)"); continue }
        let date = captureDate(props(src)) ?? "-"
        print("\((path as NSString).lastPathComponent): thumb \(th.width)x\(th.height) \(ms(t)) date=\(date)")
    }
    print("TOTAL \(paths.count) files: \(ms(total))")
}

func encode(_ inPath: String, _ outPath: String) {
    guard let src = source(inPath), let img = CGImageSourceCreateImageAtIndex(src, 0, nil) else { print("FAIL read"); return }
    // Re-render into a 16-bit buffer, as an edited image coming out of the GPU pipeline would be.
    let cs = img.colorSpace ?? CGColorSpace(name: CGColorSpace.sRGB)!
    let ctx = CGContext(data: nil, width: img.width, height: img.height, bitsPerComponent: 16, bytesPerRow: 0, space: cs,
                        bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue | CGBitmapInfo.byteOrder16Little.rawValue)!
    ctx.draw(img, in: CGRect(x: 0, y: 0, width: img.width, height: img.height))
    let img16 = ctx.makeImage()!
    let t = now()
    let url = URL(fileURLWithPath: outPath) as CFURL
    guard let dst = CGImageDestinationCreateWithURL(url, UTType.heic.identifier as CFString, 1, nil) else { print("FAIL dest"); return }
    var meta = props(src)
    meta[kCGImageDestinationLossyCompressionQuality as String] = 0.9
    meta[kCGImagePropertyDepth as String] = 10 // request 10-bit output
    CGImageDestinationAddImage(dst, img16, meta as CFDictionary)
    let ok = CGImageDestinationFinalize(dst)
    print("encode ok=\(ok) \(ms(t))")
    if let back = source(outPath) {
        let p = props(back)
        let size = (try? FileManager.default.attributesOfItem(atPath: outPath)[.size] as? Int) ?? 0
        let model = (p[kCGImagePropertyTIFFDictionary as String] as? [String: Any])?[kCGImagePropertyTIFFModel as String] ?? "-"
        print("readback depth=\(p[kCGImagePropertyDepth as String] ?? "?") size=\(size / 1024) KB date=\(captureDate(p) ?? "MISSING") model=\(model)")
    }
}

let rawExts: Set<String> = ["arw", "nef", "cr3", "cr2", "raf", "dng", "orf", "rw2"]
let displayExts: Set<String> = ["hif", "heif", "heic", "jpg", "jpeg"]
let formatSubfolders: Set<String> = ["arw", "hif", "heif", "jpg", "jpeg", "raw"]

func pair(_ dir: String) {
    let fm = FileManager.default
    var files: [String] = []
    let t = now()
    for name in (try? fm.contentsOfDirectory(atPath: dir)) ?? [] {
        let full = (dir as NSString).appendingPathComponent(name)
        var isDir: ObjCBool = false
        fm.fileExists(atPath: full, isDirectory: &isDir)
        if isDir.boolValue {
            if formatSubfolders.contains(name.lowercased()) {
                for sub in (try? fm.contentsOfDirectory(atPath: full)) ?? [] { files.append((full as NSString).appendingPathComponent(sub)) }
            }
        } else {
            files.append(full)
        }
    }
    let listing = ms(t)
    var groups: [String: [String]] = [:]
    for f in files {
        let ext = (f as NSString).pathExtension.lowercased()
        guard rawExts.contains(ext) || displayExts.contains(ext) else { continue }
        let base = ((f as NSString).lastPathComponent as NSString).deletingPathExtension.lowercased()
        groups[base, default: []].append(f)
    }
    var pairs = 0, rawOnly = 0, displayOnly = 0, mismatches = 0
    let t2 = now()
    for (base, members) in groups.sorted(by: { $0.key < $1.key }) {
        let raws = members.filter { rawExts.contains(($0 as NSString).pathExtension.lowercased()) }
        let disp = members.filter { displayExts.contains(($0 as NSString).pathExtension.lowercased()) }
        if raws.isEmpty { displayOnly += 1; continue }
        if disp.isEmpty { rawOnly += 1; print("RAW only: \(base)"); continue }
        // Timestamp guard: both files must agree on DateTimeOriginal (to the second).
        let d1 = source(raws[0]).flatMap { captureDate(props($0)) }?.prefix(19)
        let d2 = source(disp[0]).flatMap { captureDate(props($0)) }?.prefix(19)
        if d1 == nil || d1 != d2 { mismatches += 1; print("MISMATCH \(base): \(d1 ?? "nil") vs \(d2 ?? "nil")") } else { pairs += 1 }
    }
    print("files=\(files.count) listing=\(listing) photos=\(groups.count) pairs=\(pairs) rawOnly=\(rawOnly) displayOnly=\(displayOnly) mismatches=\(mismatches) exifCheck=\(ms(t2))")
}

let args = Array(CommandLine.arguments.dropFirst())
switch args.first {
case "decode": decode(Array(args.dropFirst()))
case "thumbs": thumbs(Int(args[1])!, Array(args.dropFirst(2)))
case "encode": encode(args[1], args[2])
case "pair": pair(args[1])
default: print("usage: imageio_spike decode|thumbs|encode|pair …")
}
