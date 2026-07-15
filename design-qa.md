# Design QA

- Source truth: `C:\Users\30819\AppData\Local\Temp\codex-clipboard-14a25f2d-2890-4de5-963c-f19d3d18a7f6.png`
- Implementation screenshot: `C:\Users\30819\.codex\visualizations\2026\07\15\019f65ff-7cda-7330-9b8d-6b193c2f0065\widget-final-static-v020.png`
- Dynamic refraction screenshot: `C:\Users\30819\.codex\visualizations\2026\07\15\019f65ff-7cda-7330-9b8d-6b193c2f0065\widget-final-dynamic-v020.png`
- Combined full and focused comparison: `C:\Users\30819\.codex\visualizations\2026\07\15\019f65ff-7cda-7330-9b8d-6b193c2f0065\liquid-glass-comparison-v020.png`
- Viewport and state: Windows 11, 200% display scale, 360 x 244 logical pixels, expanded state, MiniMax environment credential, real synchronized usage data.

## Full comparison

The combined comparison places the reported build and the final physical-screen capture at the same displayed widget size. The final build has a fully transparent borderless WebView surface: content behind the widget remains visible through the glass, and all four pixels outside the rounded contour show the underlying desktop rather than a gray rectangular backing.

## Focused comparison

The same comparison includes enlarged top-left and bottom-right crops. The final contour stays rounded and continuous, the cool outer rim and warm inner rim remain inside the contour, and no native title bar, square shadow, or opaque corner fill is visible. The dynamic capture also verifies the localized cool caustic and darker counter-lobe at the pointer without reducing text contrast.

## Iteration history

1. Native DWM blur was rejected after physical capture because it filled the whole HWND black and left rectangular corners.
2. Native acrylic was rejected because it produced an opaque gray surface and a visible non-client strip.
3. DWM corner preference was rejected because it reintroduced title-bar artifacts on the borderless window.
4. The final approach uses a transparent Tauri/WebView window plus clipped CSS glass layers, a dual-gradient rim, inner specular highlights, and pointer-coalesced dynamic caustics.
5. Surface alpha was tuned through 0.48, 0.68, 0.78, 0.84, and 0.88 physical-screen captures; 0.88 retained real background transmission while restoring legibility on light and dark content.

final result: passed
