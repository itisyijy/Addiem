<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="design/logo/addiem-lockup-dark.svg">
    <img src="design/logo/addiem-lockup.svg" alt="Addiem" width="300">
  </picture>
</p>

<p align="center"><b>Not just your next review date: Addiem plans how you finish the material before your exam.</b></p>

<p align="center">
  <a href="https://github.com/itisyijy/Addiem/releases/latest"><img src="https://img.shields.io/badge/version-0.1.1-4433ff" alt="Version 0.1.1"></a>
  <img src="https://img.shields.io/badge/Windows-10%20·%2011-16151f" alt="Windows 10 · 11">
  <img src="https://img.shields.io/badge/deck%20files-.apkg%20·%20.colpkg-16151f" alt="Works with .apkg · .colpkg">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-16151f" alt="MIT license"></a>
</p>

<p align="center"><a href="README.md">한국어</a> · English</p>

![The home screen with 39 cards left today, and the study screen showing card 7 of 39 with its linked lecture slide](docs/images/readme/hero.png)

**Addiem** is a flashcard app for exam preparation that reads and writes `.apkg` and `.colpkg` deck files. It manages your exam goal and the quality of your cards together, so you learn the core material first and finish on time. The name comes from the Latin *ad diem*, "until the day". In the logo, the diagonal crosses a grid of days and reaches the orange dot: your exam day.

The app's interface is in Korean.

**[Download the installer](https://github.com/itisyijy/Addiem/releases/latest)** · [Install](#install) · [Getting started](#getting-started) · [Data and privacy](#data-and-privacy) · [Known limitations](#known-limitations)

## Overview

- **Keep your existing decks.** Import `.apkg` and `.colpkg`, including review history. Or create a new deck and start right away.
- **Set an exam date and importance per deck.** Addiem picks today's cards from the days left and each part's importance (core · normal · supplementary). Review intervals use FSRS.
- **Link lecture material to cards.** Addiem reads highlights and headings from PDF and PPTX files, so you can open the slide a card came from while studying.
- **Stored on your computer.** No account needed. Studying, importing and card checks work offline. Google Drive sync and ChatGPT are optional.
- **See changes before they happen.** Importance suggestions, formatting fixes and deck updates show what will change first, and you confirm. Changes can be undone.

## Features

### Study

The home screen shows the cards left today (reviews and new) and how many come from each deck. Press **Start today's study** and rate each card Again, Hard, Good or Easy. A whole session works from the keyboard (shortcuts are configurable).

- A card you answer Again twice in one session is set aside so the rest come first. One hard card never blocks the day.
- Built-in note types: basic, both directions, cloze, written answer, list, multiple choice and image occlusion. Sound, template `{{tts}}` read-aloud and math (KaTeX) work too.
- Light, dark or follow-system theme. In Windows high-contrast themes, the logo and text use system colors.

<p>
  <img src="docs/images/readme/home.png" alt="Home with the cards left today and each deck's share" width="49%">
  <img src="docs/images/readme/home-dark.png" alt="Home in the dark theme" width="49%">
</p>

### Plans and exams

Give a deck an exam date and Addiem divides the remaining new cards by the days left (for example, 186 new cards ÷ 24 days = 8 a day). The formula and your progress are always shown.

- Study plan: expected reviews and new cards per day, daily re-planning from your actual pace, weekday and weekend study time, and conflict handling when several exams overlap.
- Pre-exam review (from 14 days before): **Compact** goes quickly through core parts, weak chapters and shaky cards; **Mixed** goes through the whole range and slips in the weak cards.
- Past-exam marks: tag cards or source sections with the exams they appeared in, and those cards come first within the same importance.

![A TOEIC vocabulary deck with its exam in 24 days, the share of started cards and the daily quota formula](docs/images/readme/deck-goal.png)

### Source material and importance

Add lecture PDFs or PPTX files to a deck. Addiem reads highlights, underlines, red text, bold text and ★ marks as **emphasized sections**, and heading structure as the **outline**. Sections whose titles contain words like "summary" or "core", or whose titles are highlighted, underlined, red or starred, are suggested as the **core range**. These are suggestions; you confirm importance yourself.

- Link an emphasized section to a card, and while studying the **Linked source** button (shortcut S) opens that page.
- Emphasized sections with no card yet are listed together, so missed material is easy to spot.
- Select a figure or table in a PDF to make an image card or an image-occlusion card.

<p>
  <img src="docs/images/readme/source-outline.png" alt="The source screen with a PDF outline and a core-range suggestion" width="49%">
  <img src="docs/images/readme/study-source.png" alt="Study screen with the linked-source panel open beside the answer" width="49%">
</p>

Importance is set per sub-deck, not per card. Addiem suggests a first layout from imported tags (such as `중요도::최우선`, "top priority"), sub-deck names (such as "요약", summary, or "부록", appendix) and source and past-exam marks. Hover a chip to see the reason in one line. Values you confirm are never overwritten by suggestions.

![The importance screen with sub-decks sorted into core, normal and supplementary](docs/images/readme/importance.png)

### Deck file compatibility and deck updates

- Import: `.apkg` and `.colpkg` (deck structure, note types, tags, media, review history), JSON and CSV. Anything that can't be handled is reported on the result screen, never dropped silently. Source files are only read.
- Export: decks as `.apkg` (with or without review history) or CSV.
- **Deck updates:** when you get a new version of a deck you already imported, Addiem offers **Import as update**. Notes the author changed are updated; your own field edits, review history and schedule are kept. If you both changed the same field, you see them side by side and choose. A backup is made first, and you can undo just that update later.

![An update summary showing changed notes, a conflict and new notes before applying](docs/images/readme/deck-update.png)

To make cards with JSON, see the [JSON bulk import format](docs/engineering/json-import-format.md) (Korean).

### Card quality

Sometimes you keep failing a card because of how it's written, not because of your memory. Card check finds such cards with rules, on your computer:

- Answers that are too long, or cards that ask several things → **Suggest a split**. Long cards that got Again twice or more in the last 14 days are listed separately.
- Questions and answers that don't seem to match (for example, it asks for a "difference" but the answer is one word) → flagged with a one-line reason.
- Numbers or items run together on one line → **Fix formatting**: the words stay the same; only line breaks, bold and separators change.

Before applying, the current and fixed cards are shown side by side, and fixes can be undone for 7 days.

![The fix-formatting dialog comparing the current card with the fixed card](docs/images/readme/card-check.png)

### Written answers

For written-answer cards, type your answer and press **Compare with answer**. Addiem marks which of the registered keywords appear in your answer and recommends a rating from them. You choose the final rating.

- **Concept relation check (experimental):** download the model (about 1.1 GB) under Settings › Written answers, and Addiem looks for the relations in the model answer (cause → effect and so on) in your answer, showing "looks right / not found". It runs only on your computer, can be wrong, and is never used for the rating recommendation. Needs a CPU with AVX2.
- If ChatGPT is connected, **Check with ChatGPT** gives a second look at the meaning.

![A written-answer card with keyword checks, your answer next to the model answer, concept relation results and the linked source](docs/images/readme/written-answer.png)

### Note types

Create and edit note types (the templates that make cards). In the **easy editor**, drag fields onto the front and back, choose size and alignment, and preview with a real note. Use the **HTML** tab to edit HTML and CSS directly. Add card types (for example word → meaning and meaning → word) to get one more card per note.

![The note type editor with fields, front and back layout, and a preview](docs/images/readme/note-type-editor.png)

### Google Drive sync

Back up to your own Google Drive automatically and continue on another computer with the same decks, cards and review history. Nothing passes through an Addiem server.

- Only two permissions: files Addiem created (`drive.file`) and an app-only folder (`drive.appdata`). Other files in your Drive are never read.
- Turn on **Lock with a password** to encrypt on your computer before uploading, so even Google can't read the content. If you forget the password, the data in Drive can't be opened (the data on your computer stays).
- Disconnecting leaves the data on your computer as it is. Without Drive, make and restore backup files under Settings › Backups on this device.

### Optional AI (ChatGPT)

Connect your ChatGPT account to draft cards from your sources, get help grading written answers, group similar content, suggest wrong options for multiple choice, and check question–answer pairs in card check. It uses your ChatGPT plan; Addiem charges nothing extra. Requests are sent only when you press a button. Everything else works without it.

![Settings with the Google Drive and ChatGPT connection sections](docs/images/readme/settings-drive-ai.png)

## Install

**Requirements**

- Windows 10 or 11 (64-bit)
- Microsoft Edge WebView2 Runtime. Windows 11 includes it; otherwise the installer downloads it (internet needed).

**Installer:** download `Addiem_0.1.1_x64-setup.exe` from [Releases](https://github.com/itisyijy/Addiem/releases/latest). Use `SHA256SUMS.txt` from the same page to check the file is intact:

```powershell
Get-FileHash .\Addiem_0.1.1_x64-setup.exe -Algorithm SHA256
```

1. Run the installer. It installs for the current user without admin rights (`%LOCALAPPDATA%\Addiem`). The installer is shown in Korean or English, following the Windows display language.
2. The app isn't code-signed yet, so a blue "Windows protected your PC" window may appear. Click **More info** → **Run anyway**.
3. Open Addiem from the Start menu.

- **If you used a version from before the rename:** Installing removes the old program first (your study data stays). On first launch Addiem moves your study data, backup files, Drive backup folder and sign-ins over to the new name and opens with the same data.
- **Updates:** there's no auto-update. Install the new version over the old one; study data is not deleted. The current version is shown at the bottom of Settings.
- **Uninstall:** remove Addiem in Windows Settings › Apps. Study data stays. To delete it too, tick **Delete the application data** in the uninstaller.
- If antivirus software (such as Microsoft Defender) blocks or removes the concept-relation engine (`llm\addiem-llm.exe`), only that feature turns off (Settings › Written answers says the engine isn't installed). Keyword grading and everything else keep working.

## Getting started

1. On the first screen, create a deck with **New deck** (just a name), or bring in a deck file (`.apkg`, `.colpkg`) or a JSON or CSV file with **Import**.
2. Set the exam date under the deck's **Goal and study settings**. Without one, the deck keeps reviewing with no deadline.
3. In **Adjust importance**, check the suggested core · normal · supplementary layout and move only what's wrong. You can skip this and still study.
4. If you have lecture PDFs or PPTX files, add them under the deck's **Sources**.
5. On the home screen, press **Start today's study** or the Enter key.

## Data and privacy

- Decks, cards, review history, media and added sources are stored on your computer (`%APPDATA%\io.github.itisyijy.addiem`). There is no Addiem server.
- The internet is used only in these cases, each one turned on or pressed by you:
  - **Google Drive sync:** uploads decks, cards, review history, settings, images and media, with only the `drive.file` and `drive.appdata` permissions. Source files (PDF, PPTX) and model files are not uploaded.
  - **ChatGPT (optional AI):** sends only the card text and source excerpts shown on screen at that moment to OpenAI. Whole source files, images, review history and deck names are not sent.
  - **Model downloads:** the analysis model (multilingual-e5-small, about 490 MB, for finding similar cards and linking sources to cards) and the concept relation model (Qwen2.5-1.5B-Instruct, 4-bit, about 1.1 GB) are downloaded from Hugging Face only when you press **Download**. Each file's size and SHA-256 are checked, and afterwards they run only on your computer.
- Google and ChatGPT sign-ins and the sync lock key are stored only in Windows Credential Manager.
- Read-aloud uses a voice that runs on your computer.

## Known limitations

- **Not code-signed.** SmartScreen warns when you first run the installer and the app (see [Install](#install)).
- **Windows only.** No macOS, Linux or mobile apps.
- **No auto-update.** Download and install new versions yourself.
- **Google sign-in is limited to registered test users** until the Google OAuth consent screen is published.
- **Concept relation check is experimental.** It's for reference and can be wrong. It needs a separate 1.1 GB download and a CPU with AVX2 (including FMA and F16C).
- Add-ons from other flashcard apps aren't supported, and scripts (JavaScript) in card templates don't run. Cards that use them are reported in the import result.
- No direct sync with other flashcard apps' sync services. Exchange decks with them as `.apkg` files.

## FAQ

**Can I use it alongside another flashcard app?**
Yes. Addiem only reads the deck files you import and never changes them, and you can export a deck as `.apkg` back to that app. Review history isn't merged automatically between the two apps, so study each deck in one app.

**What happens after the exam date?**
The deck keeps reviewing on FSRS intervals like a deck without an exam date. Set a new date and the plan is divided again.

**How do I move my data to another computer?**
Turn on Google Drive sync, or make a backup under Settings › Backups on this device and use **Restore from backup** on the other computer.

## Documentation

Design and engineering documents are in Korean.

- [Product direction](docs/product/product-direction.md) · [MVP scope](docs/product/mvp-scope.md) · [MVP screen flow](docs/product/mvp-screen-flow.md)
- [JSON bulk import format](docs/engineering/json-import-format.md)
- [Deck updates](docs/engineering/deck-update.md) · [Note types](docs/engineering/note-types.md) · [Google Drive sync design](docs/engineering/drive-sync.md)
- [UI copy guide](docs/design/copy-guide.md) · [Logo and brand](design/logo/README.md)
- [Development](docs/engineering/development.md): build, run, checks, release checklist, structure, tools

## Built with

[Tauri](https://tauri.app) · [React](https://react.dev) · [FSRS](https://github.com/open-spaced-repetition/fsrs-rs) (BSD-3-Clause) · [KaTeX](https://katex.org) (MIT) · [PDF.js](https://mozilla.github.io/pdf.js/) (Apache-2.0) · [DOMPurify](https://github.com/cure53/DOMPurify) · [Pretendard](https://github.com/orioncactus/pretendard) (OFL-1.1) · wordmark lettering from [Newsreader](https://github.com/productiontype/Newsreader) (OFL-1.1) · read-aloud voice [Supertonic](https://huggingface.co/Supertone/supertonic-3) (OpenRAIL-M) · [ONNX Runtime](https://onnxruntime.ai) (MIT) · downloadable models [multilingual-e5-small](https://huggingface.co/intfloat/multilingual-e5-small) (MIT) and [Qwen2.5-1.5B-Instruct](https://huggingface.co/Qwen/Qwen2.5-1.5B-Instruct) (Apache-2.0)

## License

[MIT](LICENSE)

---

[Website](https://addiem.pages.dev/) · [Privacy Policy](https://addiem.pages.dev/privacy/) · [Terms of Service](https://addiem.pages.dev/terms/)
