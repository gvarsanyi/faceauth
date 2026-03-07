#pragma once

#include <QObject>
#include <QStringList>
#include <QTimer>
#include <QVariantList>
#include <QtQmlIntegration>

#include <QtGlobal> // quint32

class FaceAuthBackend : public QObject
{
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Instantiated by C++")

    // --- Model state ---
    Q_PROPERTY(bool enrolled     READ enrolled     NOTIFY modelChanged)
    Q_PROPERTY(QString username  READ username     CONSTANT)
    Q_PROPERTY(int cameraIndex    READ cameraIndex NOTIFY modelChanged)
    Q_PROPERTY(QVariantList cameras READ cameras   NOTIFY camerasChanged)

    // --- Services opt list ---
    Q_PROPERTY(QVariantList servicesList READ servicesList NOTIFY servicesChanged)

    // --- Enrollment state machine ---
    Q_PROPERTY(bool enrolling    READ enrolling    NOTIFY enrollingChanged)
    Q_PROPERTY(int  capturesDone READ capturesDone NOTIFY capturesDoneChanged)
    Q_PROPERTY(int  capturesTotal READ capturesTotal CONSTANT)

    // --- Test authentication ---
    Q_PROPERTY(bool testing READ testing NOTIFY testingChanged)

    // --- Daemon availability ---
    Q_PROPERTY(bool daemonAvailable READ daemonAvailable NOTIFY daemonAvailableChanged)

    // --- Async operation status ---
    Q_PROPERTY(bool busy         READ busy         NOTIFY busyChanged)
    Q_PROPERTY(QString lastError READ lastError    NOTIFY lastErrorChanged)

public:
    static constexpr int CAPTURES_REQUIRED = 5;
    // Per-capture timeout sent to the daemon (seconds).
    static constexpr int CAPTURE_TIMEOUT_SECS = 30;

    explicit FaceAuthBackend(QObject *parent = nullptr);

    // Property accessors
    bool        enrolled()      const { return m_batchCount > 0; }
    QString     username()      const { return m_username; }
    int         cameraIndex()   const { return m_cameraIndex; }
    QVariantList cameras()        const { return m_cameras; }
    QVariantList servicesList()   const { return m_servicesList; }
    bool        daemonAvailable() const { return m_daemonAvailable; }
    bool        enrolling()     const { return m_enrolling; }
    int         capturesDone()  const { return m_capturesDone; }
    int         capturesTotal() const { return CAPTURES_REQUIRED; }
    bool        testing()       const { return m_testing; }
    bool        busy()          const { return m_busy; }
    QString     lastError()     const { return m_lastError; }

Q_SIGNALS:
    void modelChanged();
    void camerasChanged();
    void servicesChanged();
    void daemonAvailableChanged();
    void enrollingChanged();
    void capturesDoneChanged();
    void busyChanged();
    void lastErrorChanged();
    void testingChanged();

    // Emitted after all captures complete and the Enroll request succeeds.
    void enrollmentSucceeded();
    // Emitted on any operation error (sets lastError first).
    void operationFailed(const QString &message);
    // Emitted after a testAuth() call completes.
    void authSucceeded();
    void authFailed(const QString &message);
    // Emitted by the "Enroll New Face..." button action. Standalone wires this
    // to open a dialog; KCM wires it to kcm.push("EnrollPage.qml", ...).
    void enrollRequested();

public Q_SLOTS:
    // Load current user's model from the daemon; populates all properties.
    Q_INVOKABLE void refresh();

    // Service opt-in/opt-out (immediate apply, updates <uid>.opt).
    Q_INVOKABLE void setServiceOpt(const QString &service, bool allowed);

    // Clear all enrollment data.
    Q_INVOKABLE void clearAll();

    // Enrollment flow. cameraIndex is the /dev/videoN index to use.
    // Automatically performs CAPTURES_REQUIRED CaptureEncoding round-trips,
    // then submits a single Enroll request. No threads needed  -  uses the
    // Qt event loop via signal-chained async callbacks.
    Q_INVOKABLE void startEnrollment(int cameraIndex);
    Q_INVOKABLE void cancelEnrollment();

    // Run a single authentication attempt against the enrolled model.
    // Emits authSucceeded() or authFailed(message) when complete.
    Q_INVOKABLE void testAuth();

    // Clear the lastError property (called from QML when the user dismisses
    // the error banner).
    Q_INVOKABLE void clearLastError();

private:
    // Send a quick daemon request and call refresh() on success, or set
    // lastError on a non-connection failure. Used by setServiceOpt() and clearAll().
    void sendAndRefresh(QJsonObject request);
    // Fetch the merged service list and populate m_servicesList.
    void getServices();
    void captureOne();
    void submitEnrollment();
    void parseModelJson(const QString &rawJson);
    void listCameras();
    // Desktop notification helpers for testAuth(). Both are fire-and-async;
    // notifyStart() spawns the helper and stores the ID in m_notifId once done.
    void notifyStart();
    void notifyFinish(quint32 notifId, bool success);
    // Lightweight daemon health-check used by the poll timer. Uses ListCameras
    // as the probe but intentionally ignores the returned data  -  camera data is
    // only loaded at startup and after a down->up recovery.
    void pingDaemon();
    void setBusy(bool b);
    void setLastError(const QString &err);
    // Called from every daemon callback. Tracks whether the daemon is reachable.
    void checkAvailability(bool ok, bool connectionError);

    // Current user (resolved at construction time via getpwuid/getuid).
    QString m_username;
    quint32 m_uid = 0;

    // Model state  -  updated by parseModelJson() after each daemon call.
    int         m_batchCount = 0;

    // Services opt list  -  updated by getServices().
    QVariantList m_servicesList;

    // Available cameras  -  populated once at startup by listCameras().
    QVariantList m_cameras;

    // Enrollment state machine.
    bool     m_enrolling     = false;
    int      m_capturesDone  = 0;
    bool     m_testing       = false;
    quint32  m_notifId       = 0;   // notification ID from notifyStart(); 0 = none
    int      m_cameraIndex   = 0;
    // Each element is a QVariantList of 128 doubles (one capture).
    QList<QVariantList> m_capturedEncodings;

    // Daemon availability  -  false when the socket is unreachable.
    bool    m_daemonAvailable = true; // optimistic; corrected on first response
    QTimer *m_pollTimer = nullptr;    // health-checks daemon every 2 s (always running)

    // Async status.
    bool    m_busy      = false;
    QString m_lastError;
};
