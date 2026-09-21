function (id) {
    'use strict';
    if (['browse-open', 'drc-open', 'about-open'].indexOf(id) < 0) return 'unavailable';
    if (document.hidden) return 'hidden';
    // Do not replace an approval, unsaved editor, or exit confirmation.
    if (document.querySelector('[role="dialog"]:not([hidden])')) return 'busy';
    var button = document.getElementById(id);
    if (!button || button.disabled || button.hidden) return 'unavailable';
    button.click();
    return 'opened';
}
