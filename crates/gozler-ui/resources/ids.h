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
#define IDC_MESSAGE_BUTTON_1 18
#define IDC_MESSAGE_BUTTON_2 19

// The filter pane, a window of its own that scrolls, and its controls.
#define IDC_FILTER_PANE 20
#define IDC_CLEAR_FILTERS 21
#define IDC_KIND_LABEL 22
#define IDC_KIND 23
#define IDC_LOCATION_LABEL 24
#define IDC_LOCATION 25
#define IDC_CHOOSE_FOLDER 26
#define IDC_ONLY_DIRECTLY 27
#define IDC_DATE_LABEL 28
#define IDC_DATE 29
#define IDC_SIZE_LABEL 30
#define IDC_SIZE 31
#define IDC_ATTRIBUTES_LABEL 32
#define IDC_HIDDEN_LABEL 33
#define IDC_HIDDEN 34
#define IDC_SYSTEM_LABEL 35
#define IDC_SYSTEM 36
#define IDC_READ_ONLY_LABEL 37
#define IDC_READ_ONLY 38
#define IDC_LINK_LABEL 39
#define IDC_LINK 40
#define IDC_COMPRESSED_LABEL 41
#define IDC_COMPRESSED 42
#define IDC_ENCRYPTED_LABEL 43
#define IDC_ENCRYPTED 44

// The inspector's controls; each field row is a label and a read-only value.
#define IDC_INSPECTOR_HEADING 50
#define IDC_INSPECTOR_OPEN 51
#define IDC_INSPECTOR_OPEN_FILE_LOCATION 52
#define IDC_INSPECTOR_COPY_AS_PATH 53
#define IDC_INSPECTOR_LABEL_1 54
#define IDC_INSPECTOR_VALUE_1 55
#define IDC_INSPECTOR_LABEL_2 56
#define IDC_INSPECTOR_VALUE_2 57
#define IDC_INSPECTOR_LABEL_3 58
#define IDC_INSPECTOR_VALUE_3 59
#define IDC_INSPECTOR_LABEL_4 60
#define IDC_INSPECTOR_VALUE_4 61
#define IDC_INSPECTOR_LABEL_5 62
#define IDC_INSPECTOR_VALUE_5 63
#define IDC_INSPECTOR_LABEL_6 64
#define IDC_INSPECTOR_VALUE_6 65
#define IDC_INSPECTOR_LABEL_7 66
#define IDC_INSPECTOR_VALUE_7 67

#define IDD_ABOUT 100
#define IDC_ABOUT_ICON 1001
#define IDC_ABOUT_VERSION 1002
#define IDC_ABOUT_DESCRIPTION 1003
#define IDC_ABOUT_COPYRIGHT 1004
#define IDC_ABOUT_LINK 1005

#define IDD_SEARCH_SYNTAX 101
#define IDC_SEARCH_SYNTAX_TEXT 1101

#define IDD_CUSTOM_DATE 102
#define IDC_CUSTOM_DATE_FROM_LABEL 1201
#define IDC_CUSTOM_DATE_FROM 1202
#define IDC_CUSTOM_DATE_TO_LABEL 1203
#define IDC_CUSTOM_DATE_TO 1204

#define IDD_CUSTOM_SIZE 103
#define IDC_CUSTOM_SIZE_MINIMUM_LABEL 1301
#define IDC_CUSTOM_SIZE_MINIMUM 1302
#define IDC_CUSTOM_SIZE_MAXIMUM_LABEL 1303
#define IDC_CUSTOM_SIZE_MAXIMUM 1304
#define IDC_CUSTOM_SIZE_UNIT_LABEL 1305
#define IDC_CUSTOM_SIZE_UNIT 1306
