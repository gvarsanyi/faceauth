// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: faceauth contributors

pragma ComponentBehavior: Bound

import QtQuick
import org.kde.kirigami as Kirigami
import org.kde.ki18n
import org.faceauth.kcm 1.0  // provides the Backend singleton

Kirigami.ApplicationWindow {
    id: appWindow

    title: i18n("Face Authentication")
    width: Kirigami.Units.gridUnit * 40
    height: Kirigami.Units.gridUnit * 36
    minimumWidth: Kirigami.Units.gridUnit * 28
    minimumHeight: Kirigami.Units.gridUnit * 24

    // Instantiate main.qml and override 'backend' with the singleton so
    // that 'kcm' is never referenced in the standalone path.
    Component.onCompleted: {
        pageStack.push(Qt.resolvedUrl("main.qml"), { backend: Backend })
    }
}
