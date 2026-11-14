# Bedfont

Bedfont is bEd's application-facing alias for unmodified Nordwand Mono 1.3.3,
by Tanguy VIVIER. Original font metadata and filenames remain intact.

- Source: https://github.com/tywr/Nordwand-Mono/releases/tag/1.3.3
- Archive: https://github.com/tywr/Nordwand-Mono/releases/download/1.3.3/NordwandMono-TTF.zip
- Archive SHA-256: `d9955d0b1b924e11fd822ceb23986a1253d83b00d9e66aa97c6439c6ccfb8913`
- `NordwandMono-Regular.ttf` SHA-256: `d2fcf9df86a6fb93274ad76d1f8261d6dce8250d781aeaef1c41c2f9d8df8fbd`
- `NordwandMono-Bold.ttf` SHA-256: `9ca68a6b7fbc4ef00a2dd989c2d2143f8cb8e0eb328c484469513f68c844e5e4`
- License: [SIL Open Font License 1.1](NordwandMono-OFL.txt), retained from
  https://raw.githubusercontent.com/tywr/Nordwand-Mono/1.3.3/LICENSE-FONT.txt

Only regular and bold are embedded. UI retains egui's bundled Unicode fallbacks;
the host application owns installed font selection. bEd packages include this
record and the original font license in their notices.

# DejaVu Sans Mono (symbol fallback)

Unmodified DejaVu Sans Mono 2.37, placed after Bedfont and Hack and ahead of
egui's emoji fonts. It draws dingbats and other symbols the text fonts lack,
such as TUI spinner stars (`✢ ✶ ✻ ✽`) and check marks (`✓ ✗`).

- Source: https://github.com/dejavu-fonts/dejavu-fonts/releases/tag/version_2_37
- Archive: https://github.com/dejavu-fonts/dejavu-fonts/releases/download/version_2_37/dejavu-fonts-ttf-2.37.tar.bz2
- Archive SHA-256: `fa9ca4d13871dd122f61258a80d01751d603b4d3ee14095d65453b4e846e17d7`
- `DejaVuSansMono.ttf` SHA-256: `b4a6c3e4faab8773f4ff761d56451646409f29abedd68f05d38c2df667d3c582`
- License: Bitstream Vera / Arev fonts license, with DejaVu changes in the public
  domain ([DejaVuSansMono-LICENSE.txt](DejaVuSansMono-LICENSE.txt)), retained
  from the archive's `LICENSE`.
