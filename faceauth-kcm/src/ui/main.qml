// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: Greg Varsanyi

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

    // The enrolled camera is connected (or nothing is enrolled yet).
    readonly property bool cameraReady: root.backend.enrolled ? root.backend.cameraIndex >= 0 : root.backend.cameras.length > 0

    title: i18n("Face Authentication")

    // --- Test state ---
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
        target: root.backend
        function onEnrollingChanged() {
            // Neither success nor error: cancelled, or the password prompt
            // was dismissed. Nothing more to show.
            if (!root.backend.enrolling && !enrollmentSheet.capturingFinished && root.backend.lastError.length === 0)
                enrollmentSheet.close();
        }
        function onEnrollmentSucceeded() {
            enrollmentSheet.capturingFinished = true;
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
            text: i18n("The face authentication service (faceauth-daemon) is not available")
            actions: [
                Kirigami.Action {
                    text: i18n("Retry")
                    icon.name: "view-refresh"
                    onTriggered: root.backend.refresh()
                }
            ]
        }

        // Closing clears the error; visibility follows lastError, so the
        // binding is never overwritten.
        Kirigami.InlineMessage {
            Layout.fillWidth: true
            type: Kirigami.MessageType.Error
            visible: root.backend.lastError.length > 0
            text: root.backend.lastError
            showCloseButton: false
            actions: [
                Kirigami.Action {
                    icon.name: "dialog-close"
                    displayHint: Kirigami.DisplayHint.IconOnly
                    text: i18n("Close")
                    onTriggered: root.backend.clearLastError()
                }
            ]
        }

        Kirigami.FormLayout {
            visible: root.backend.daemonAvailable

            // --- Status ---
            Kirigami.Separator {
                Layout.fillWidth: true
                Kirigami.FormData.isSection: true
                Kirigami.FormData.label: i18n("Status")
            }

            Controls.Label {
                Kirigami.FormData.label: i18n("Service:")
                text: root.backend.enrolled ? i18n("Available") : i18n("Available, no face model data")
                color: root.backend.enrolled ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.neutralTextColor
            }

            Controls.Button {
                text: i18n("Test Face Authentication")
                icon.name: "webcam"
                enabled: root.backend.enrolled && root.cameraReady
                onClicked: {
                    // Reset before the call: a quick failure may arrive
                    // before the dialog's open animation ends.
                    root.testSuccess = false;
                    root.testError = "";
                    root.testElapsedSecs = 0;
                    testProgressTimer.start();
                    testSheet.open();
                    root.backend.testAuth();
                }
            }

            // --- Enrollment ---
            Kirigami.Separator {
                Layout.fillWidth: true
                Kirigami.FormData.isSection: true
                Kirigami.FormData.label: i18n("Enrollment")
            }

            // Camera: a selector before the first enrollment, then the
            // enrolled camera (more batches always use it).
            RowLayout {
                Kirigami.FormData.label: i18n("Camera:")
                spacing: Kirigami.Units.smallSpacing

                Controls.ComboBox {
                    id: cameraCombo
                    Layout.fillWidth: true
                    model: root.backend.cameras
                    textRole: "name"
                    valueRole: "index"
                    visible: !root.backend.enrolled
                    // Default to the most suitable (IR) camera; ties go to the
                    // lowest index since the list is index-sorted.
                    currentIndex: {
                        var best = 0;
                        for (var i = 1; i < root.backend.cameras.length; i++) {
                            if (root.backend.cameras[i].suitability > root.backend.cameras[best].suitability)
                                best = i;
                        }
                        return best;
                    }
                }

                Controls.Label {
                    visible: root.backend.enrolled
                    text: {
                        for (var i = 0; i < root.backend.cameras.length; i++) {
                            if (root.backend.cameras[i].index === root.backend.cameraIndex)
                                return root.backend.cameras[i].name;
                        }
                        return root.backend.cameraIndex >= 0 ? "/dev/video" + root.backend.cameraIndex : i18n("Enrolled camera not connected");
                    }
                }
            }

            RowLayout {
                Kirigami.FormData.label: i18n("Face Model Data:")
                spacing: Kirigami.Units.smallSpacing

                Controls.Button {
                    text: root.backend.enrolled ? i18n("Add More") : i18n("Add")
                    icon.name: "list-add-user"
                    enabled: root.cameraReady
                    onClicked: {
                        // When enrolled, the backend reuses the model's camera.
                        root.backend.startEnrollment(cameraCombo.currentValue);
                        enrollmentSheet.open();
                    }
                }

                Controls.Button {
                    text: i18n("Delete All")
                    icon.name: "edit-delete"
                    visible: root.backend.enrolled
                    onClicked: clearConfirmDialog.open()
                }
            }

            // --- Services ---
            Kirigami.Separator {
                Layout.fillWidth: true
                Kirigami.FormData.isSection: true
                Kirigami.FormData.label: i18n("Services")
            }

            Controls.Label {
                Kirigami.FormData.label: i18n("Never used for:")
                text: i18n("Remote sessions (SSH, remote desktop), including sudo inside them")
                enabled: false
            }

            Controls.Label {
                text: i18n("Background processes and sessions that are not in the foreground")
                enabled: false
            }

            ColumnLayout {
                Kirigami.FormData.label: i18n("Use for:")
                Kirigami.FormData.labelAlignment: Qt.AlignTop
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing

                Controls.Label {
                    text: i18n("(no services enabled by the administrator)")
                    visible: root.backend.servicesList.length === 0
                    enabled: false
                }

                Controls.Label {
                    Layout.fillWidth: true
                    wrapMode: Text.WordWrap
                    text: i18n("The administrator chooses the available services in /etc/faceauth/faceauth.conf.")
                    enabled: false
                    font: Kirigami.Theme.smallFont
                }

                Repeater {
                    model: root.backend.servicesList
                    delegate: Controls.CheckBox {
                        required property var modelData
                        text: modelData.name
                        checked: modelData.enabled
                        onToggled: root.backend.setServiceEnabled(modelData.name, checked)
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
        // The daemon check cannot be cancelled; wait for its result.
        closePolicy: root.backend.testing ? Controls.Popup.NoAutoClose : Controls.Popup.CloseOnEscape

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
                value: root.testElapsedSecs
            }

            Controls.Button {
                Layout.alignment: Qt.AlignRight
                text: i18n("Close")
                icon.name: "dialog-close"
                enabled: !root.backend.testing
                onClicked: testSheet.close()
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

        onOpened: capturingFinished = false
        onClosed: {
            if (root.backend.enrolling)
                root.backend.cancelEnrollment();
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

            Controls.Button {
                Layout.alignment: Qt.AlignRight
                text: root.backend.enrolling ? i18n("Cancel") : i18n("Close")
                icon.name: root.backend.enrolling ? "dialog-cancel" : "dialog-close"
                onClicked: enrollmentSheet.close()
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
