// The resource and control IDs, the one source for gozler-ui.rc, the code
// (build.rs generates core::ids from this file) and probe.ps1. build.rs
// refuses two controls with one ID, a control with ID 1 or 2, which are
// IDOK and IDCANCEL, and a dialog that gozler-ui.rc does not define.

#define IDI_GOZLER 1

// The main window's controls, below every menu item ID.
#define IDC_SEARCH 10
#define IDC_MATCH_LABEL 11
#define IDC_MATCH_MODE 12
#define IDC_MATCH_CASE 13
#define IDC_MATCH_PATH 14
#define IDC_LIST 15
#define IDC_STATUS 16
#define IDC_MESSAGE 17
#define IDC_FIX 18

#define IDD_ABOUT 100
#define IDC_ABOUT_ICON 1001
#define IDC_ABOUT_VERSION 1002
#define IDC_ABOUT_DESCRIPTION 1003
#define IDC_ABOUT_COPYRIGHT 1004
#define IDC_ABOUT_LINK 1005

#define IDD_SEARCH_SYNTAX 101
#define IDC_SEARCH_SYNTAX_TEXT 1101
