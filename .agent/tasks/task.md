
You are assisting the user with a question about "dynamic clipping" in madVR measurement files and how it relates to `mkvdolby` and `hdr-analyze`.

The user has the following questions:
1. Does `mkvdolby` use the dynamic clipping info?
2. Should we add it? Would it improve results?

The user provided a sample file `s01e10.measurements` and a parser `madvr_parse-main`.

Plan:
1.  **Locate the files**: I will look for `s01e10.measurements` and the `madvr_parse-main` directory.
2.  **Analyze `s01e10.measurements`**: I will use the provided `madvr_parse` tool (or code) to inspect the contents of the measurements file. I want to see if the histograms or header maxCLL reflect the "clipped" values or the original values.
    *   If the histograms are modified (clipped), then `mkvdolby` (via `dovi_tool`) *is* using the info, but potentially in a dangerous way (mismatch with video).
    *   If the measurements file contains *separate* fields for dynamic clipping that `dovi_tool` ignores, then it is not being used.
3.  **Investigate `dovi_tool` / `mkvdolby` interaction**: Check how `mkvdolby` invokes `dovi_tool` and what `dovi_tool` does with the measurements file.
4.  **Formulate Answer**:
    *   Confirm whether dynamic clipping alters the base stats (L1) or is separate data.
    *   Advise on whether to implement/support it. (Likely "No" if it mismatches video, unless we are doing on-the-fly reshaping which we are not).

Task: `Analyze s01e10.measurements and Evaluate Dynamic Clipping Support`
