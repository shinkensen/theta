THETA - Portable Distribution Package
======================================

This package contains everything you need to run Theta on Windows without installation.

CONTENTS:
---------
- theta.exe                 : Main application executable
- libvosk.dll              : Vosk speech recognition library
- libgcc_s_seh-1.dll       : GCC runtime library
- libstdc++-6.dll          : C++ standard library
- libwinpthread-1.dll      : Windows pthread library
- vosk-models/             : Speech recognition model directory
  └── vosk-model-small-en-us-0.15/

INSTRUCTIONS:
-------------
1. Extract the contents of theta-portable.zip to a folder of your choice
2. Make sure ALL files and the vosk-models folder are in the same directory
3. Double-click theta.exe to run the application

REQUIREMENTS:
-------------
- Windows 10 or later (64-bit)
- No additional installation required - all dependencies are included

TROUBLESHOOTING:
----------------
If you get an error about missing DLL files:
- Make sure all .dll files are in the same folder as theta.exe
- Make sure the vosk-models folder is in the same directory
- Try running as Administrator if you encounter permission issues

If the application doesn't start:
- Check Windows Event Viewer for detailed error messages
- Ensure your antivirus isn't blocking the executable

NOTES:
------
- Total package size: ~193 MB (uncompressed)
- The vosk-model-small-en-us-0.15 is required for speech recognition
- Keep all files together - do not separate the DLLs from the executable

VERSION: 0.2.0
