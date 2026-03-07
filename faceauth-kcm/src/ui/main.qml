// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: faceauth contributors

pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.kcmutils as KCMUtils
import org.kde.ki18n

// Root item for the KCM. KQuickConfigModule injects 'kcm' as a context
// property, so QML can access kcm.backend. standalone.qml instantiates this
// file as a component and overrides the 'backend' property binding.
KCMUtils.SimpleKCM {
    id: root

    // Overridden by standalone.qml with the Backend singleton.
    // In KCM mode this is kcm.backend, injected by KQuickConfigModule.
    property var backend: typeof kcm !== "undefined" ? kcm.backend : null

    title: i18n("Face Authentication")

    // --- Enrollment state ---

    // True while the inline enrollment section is visible.
    property bool enrollmentMode: false

    // --- Test state ---
    property bool testMode: false      // true while test UI is visible
    property bool testSuccess: false
    property string testError: ""
    readonly property real testTimeoutSecs: 5
    property real testElapsedSecs: 0

    Timer {
        id: testProgressTimer
        interval: 100
        repeat: true
        onTriggered: {
            if (root.backend.testing) {
                root.testElapsedSecs = Math.min(root.testElapsedSecs + interval / 1000, root.testTimeoutSecs);
            } else {
                stop();
                root.testElapsedSecs = root.testTimeoutSecs;
            }
        }
    }

    Component.onDestruction: {
        if (root.backend.enrolling)
            root.backend.cancelEnrollment();
    }

    Connections {
        target: backend
        function onEnrollingChanged() {
            if (root.backend.enrolling)
                enrollmentSheet.capturingFinished = false;
        }
        function onEnrollmentSucceeded() {
            enrollmentSheet.capturingFinished = true;
        }
        function onOperationFailed(message) {
            enrollmentSheet.capturingFinished = false;
        }
        function onCapturesDoneChanged() {
            if (root.backend.capturesTotal > 0 && root.backend.capturesDone >= root.backend.capturesTotal) {
                enrollmentSheet.capturingFinished = true;
            }
        }
        function onAuthSucceeded() {
            root.testSuccess = true;
            root.testError = "";
            testProgressTimer.stop();
            root.testElapsedSecs = root.testTimeoutSecs;
        }
        function onAuthFailed(message) {
            root.testSuccess = false;
            root.testError = message;
            testProgressTimer.stop();
            root.testElapsedSecs = root.testTimeoutSecs;
        }
    }

    // --- Main form ---
    ColumnLayout {
        spacing: Kirigami.Units.largeSpacing
        enabled: !root.backend.busy

        // --- Banners ---
        Kirigami.InlineMessage {
            Layout.fillWidth: true
            type: Kirigami.MessageType.Error
            visible: !root.backend.daemonAvailable
            showCloseButton: false
            text: i18n("Face authentication service (faceauth-daemon.service) is not available")
        }

        Kirigami.InlineMessage {
            id: errorBanner
            Layout.fillWidth: true
            type: Kirigami.MessageType.Error
            visible: root.backend.lastError.length > 0
            text: root.backend.lastError
            showCloseButton: true
            onVisibleChanged: if (!visible)
                root.backend.clearLastError()
        }

        // Test authentication feedback is shown in the overlay.

        Kirigami.FormLayout {
            visible: root.backend.daemonAvailable

            // --- Status ---
            Kirigami.Separator {
                Layout.fillWidth: true
                Kirigami.FormData.isSection: true
                Kirigami.FormData.label: i18n("Status")
            }

            RowLayout {
                Kirigami.FormData.label: i18n("Service:")
                spacing: Kirigami.Units.smallSpacing

                Controls.Label {
                    text: root.backend.enrolled ? i18n("Available") : i18n("Available, no face model data")
                    color: root.backend.enrolled ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.neutralTextColor
                }
            }

            // Test button  -  visible when enrolled and idle.
            RowLayout {
                spacing: Kirigami.Units.smallSpacing

                Controls.Button {
                    text: i18n("Test Face Authentication")
                    icon.name: "webcam"
                    enabled: root.backend.enrolled && !root.enrollmentMode && !root.testMode
                    onClicked: {
                        testSheet.open();
                        root.backend.testAuth();
                    }
                }
            }

            // --- Enrollment ---
            Kirigami.Separator {
                Layout.fillWidth: true
                Kirigami.FormData.isSection: true
                Kirigami.FormData.label: i18n("Enrollment")
            }

            // Camera row  -  selector or info label, plus enroll button when idle.
            // Hidden entirely when the daemon is unavailable (no cameras to show).
            RowLayout {
                Kirigami.FormData.label: i18n("Camera:")
                spacing: Kirigami.Units.smallSpacing

                // Selector  -  when no model yet, or during enrollment (disabled,
                // pre-selected to the model's camera for re-enrollment).
                Controls.ComboBox {
                    id: cameraCombo
                    Layout.fillWidth: true
                    model: root.backend.cameras
                    textRole: "name"
                    valueRole: "index"
                    visible: !root.backend.enrolled
                    // Only enabled for fresh enrollment before capturing starts;
                    // re-enrollment reuses the existing camera.
                    enabled: !root.backend.enrolled && !root.backend.enrolling
                    currentIndex: {
                        // Fresh enrollment: default to the highest-suitability camera
                        // (IR cameras are best for face auth). Ties go to the first
                        // (lowest-index) camera since the list is index-sorted.
                        var best = 0;
                        for (var i = 1; i < root.backend.cameras.length; i++) {
                            if ((root.backend.cameras[i].suitability || 0) > (root.backend.cameras[best].suitability || 0))
                                best = i;
                        }
                        return best;
                    }
                }

                // Info label  -  shown when enrolled and not in enrollment mode.
                // Uses the same display name as the dropdown (path + device name);
                // falls back to cameraInfo if the cameras list isn't populated yet.
                Controls.Button {
                    text: {
                        for (var i = 0; i < root.backend.cameras.length; i++) {
                            if (root.backend.cameras[i].index === root.backend.cameraIndex)
                                return root.backend.cameras[i].name;
                        }
                        return "/dev/video" + root.backend.cameraIndex;
                    }
                    enabled: false
                    visible: root.backend.enrolled
                }
            }

            RowLayout {
                Kirigami.FormData.label: i18n("Face Model Data:")
                spacing: Kirigami.Units.smallSpacing

                Controls.Button {
                    text: root.backend.enrolled ? i18n("Add More") : i18n("Add")
                    icon.name: "list-add-user"
                    enabled: root.backend.cameras.length > 0
                    onClicked: {
                        var idx = root.backend.enrolled ? root.backend.cameraIndex : ((cameraCombo.currentValue !== undefined) ? cameraCombo.currentValue : 0);
                        root.backend.startEnrollment(idx);
                        enrollmentSheet.open();
                    }
                }

                Controls.Button {
                    text: i18n("Delete All")
                    icon.name: "edit-delete"
                    enabled: root.backend.enrolled
                    visible: root.backend.enrolled && !root.enrollmentMode
                    onClicked: clearConfirmDialog.open()
                }
            }

            // Enrollment workflow is shown in the enrollmentSheet dialog.

            // --- Services ---
            Kirigami.Separator {
                Layout.fillWidth: true
                Kirigami.FormData.isSection: true
                Kirigami.FormData.label: i18n("Services")
            }

            Controls.Label {
                Kirigami.FormData.label: i18n("Always ignored:")
                text: i18n("SSH sessions (including sudo within SSH)")
                enabled: false
            }

            ColumnLayout {
                Kirigami.FormData.label: i18n("Allowed:")
                Kirigami.FormData.labelAlignment: Qt.AlignTop
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing

                Controls.Label {
                    text: i18n("(no services recorded yet)")
                    visible: root.backend.servicesList.length === 0
                    enabled: false
                }

                Repeater {
                    model: root.backend.servicesList
                    delegate: Controls.CheckBox {
                        required property var modelData
                        text: modelData.name
                        checked: modelData.allowed
                        onToggled: root.backend.setServiceOpt(modelData.name, checked)
                    }
                }
            }
        }

        // Bottom spacer
        Item {
            Layout.fillHeight: true
        }
    }

    // --- Test dialog ---
    Kirigami.Dialog {
        id: testSheet
        title: i18n("Test Face Authentication")
        standardButtons: Kirigami.Dialog.NoButton
        padding: Kirigami.Units.largeSpacing

        onOpened: {
            root.testMode = true;
            root.testSuccess = false;
            root.testError = "";
            root.testElapsedSecs = 0;
            testProgressTimer.start();
        }
        onClosed: {
            root.testMode = false;
            root.testSuccess = false;
            root.testError = "";
            testProgressTimer.stop();
            root.testElapsedSecs = 0;
        }

        ColumnLayout {
            spacing: Kirigami.Units.mediumSpacing

            Kirigami.InlineMessage {
                Layout.preferredWidth: Kirigami.Units.gridUnit * 24
                type: Kirigami.MessageType.Error
                visible: root.testError.length > 0
                text: i18n("Authentication failed: %1", root.testError)
                showCloseButton: false
            }

            Controls.Label {
                Layout.preferredWidth: Kirigami.Units.gridUnit * 24
                wrapMode: Text.WordWrap
                text: root.backend.testing ? i18n("Look at the camera...") : root.testSuccess ? i18n("Authentication successful!") : i18n("Failed")
                color: root.testSuccess ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.textColor
            }

            Controls.ProgressBar {
                Layout.fillWidth: true
                from: 0
                to: root.testTimeoutSecs
                value: root.backend.testing ? root.testElapsedSecs : (root.testSuccess || root.testError.length > 0 ? root.testTimeoutSecs : 0)
            }

            RowLayout {
                Layout.alignment: Qt.AlignRight

                Controls.Button {
                    text: root.backend.testing ? i18n("Cancel") : i18n("Close")
                    icon.name: "dialog-ok"
                    onClicked: testSheet.close()
                }
            }
        }
    }

    // --- Enrollment dialog ---
    Kirigami.Dialog {
        id: enrollmentSheet
        title: i18n("Enroll Face")
        standardButtons: Kirigami.Dialog.NoButton
        padding: Kirigami.Units.largeSpacing

        property bool capturingFinished: false

        onOpened: {
            root.enrollmentMode = true;
            capturingFinished = false;
        }

        onClosed: {
            if (root.backend.enrolling)
                root.backend.cancelEnrollment();
            root.enrollmentMode = false;
        }

        ColumnLayout {
            spacing: Kirigami.Units.mediumSpacing

            Controls.Label {
                Layout.preferredWidth: Kirigami.Units.gridUnit * 24
                wrapMode: Text.WordWrap
                text: {
                    if (root.backend.lastError.length > 0)
                        return i18n("Enrollment failed: %1", root.backend.lastError);
                    if (enrollmentSheet.capturingFinished)
                        return i18n("Enrollment complete!");
                    return i18n("Look at the camera...");
                }
                color: root.backend.lastError.length > 0 ? Kirigami.Theme.negativeTextColor : Kirigami.Theme.textColor
            }

            Controls.Label {
                Layout.preferredWidth: Kirigami.Units.gridUnit * 24
                wrapMode: Text.WordWrap
                text: i18n("Vary your angle slightly between each capture.")
                enabled: false
            }

            Controls.ProgressBar {
                Layout.fillWidth: true
                from: 0
                to: root.backend.capturesTotal
                value: enrollmentSheet.capturingFinished ? root.backend.capturesTotal : root.backend.capturesDone
            }

            RowLayout {
                Layout.alignment: Qt.AlignRight
                spacing: Kirigami.Units.smallSpacing

                Controls.Button {
                    text: root.backend.enrolling ? i18n("Cancel") : i18n("Close")
                    icon.name: root.backend.enrolling ? "dialog-cancel" : "dialog-ok"
                    onClicked: enrollmentSheet.close()
                }
            }
        }
    }

    // --- Confirmation dialog ---
    Kirigami.PromptDialog {
        id: clearConfirmDialog
        title: i18n("Clear Face Data?")
        subtitle: i18n("All enrolled face data will be permanently removed. You will need to enroll again to use face authentication.")
        standardButtons: Kirigami.Dialog.Ok | Kirigami.Dialog.Cancel
        onAccepted: root.backend.clearAll()
    }
}
