import re

with open("faceauth-kcm/src/ui/main.qml", "r") as f:
    text = f.read()

# Kirigami.Dialog normally has its own padding, but if we're directly replacing OverlaySheet, we might need explicitly defined margins.
# Kirigami.Dialog is essentially a Controls.Dialog, so it has `padding`, `topPadding`, `bottomPadding`, etc.
# We can set `padding: Kirigami.Units.largeSpacing` on both Dialogs.

text = text.replace("""    Kirigami.Dialog {
        id: testSheet
        title: i18n("Test Face Authentication")
        standardButtons: Kirigami.Dialog.NoButton""", """    Kirigami.Dialog {
        id: testSheet
        title: i18n("Test Face Authentication")
        standardButtons: Kirigami.Dialog.NoButton
        padding: Kirigami.Units.largeSpacing""")

text = text.replace("""    Kirigami.Dialog {
        id: enrollmentSheet
        title: i18n("Enroll Face")
        standardButtons: Kirigami.Dialog.NoButton""", """    Kirigami.Dialog {
        id: enrollmentSheet
        title: i18n("Enroll Face")
        standardButtons: Kirigami.Dialog.NoButton
        padding: Kirigami.Units.largeSpacing""")

with open("faceauth-kcm/src/ui/main.qml", "w") as f:
    f.write(text)

