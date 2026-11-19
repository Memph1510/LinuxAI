// LinuxAI's always-on layer (Quickshell): a thin status line for now. Next: the voice indicator, the AI's
// questions, and its pointer, highlight and magnifier over any app.
import Quickshell
import Quickshell.Io
import QtQuick

ShellRoot {
    PanelWindow {
        anchors { top: true; left: true; right: true }
        implicitHeight: 28
        color: "#0b1016"

        Text {
            anchors.left: parent.left
            anchors.leftMargin: 14
            anchors.verticalCenter: parent.verticalCenter
            text: "Linux<font color='#46c2c9'>AI</font>"
            textFormat: Text.RichText
            color: "#dfe7ee"
            font.pixelSize: 14
            font.bold: true
        }

        Text {
            id: status
            anchors.centerIn: parent
            color: "#8ea0b1"
            font.pixelSize: 12
            text: "core: checking…"
        }

        Text {
            id: clock
            anchors.right: parent.right
            anchors.rightMargin: 14
            anchors.verticalCenter: parent.verticalCenter
            color: "#dfe7ee"
            font.pixelSize: 12
            text: Qt.formatDateTime(new Date(), "ddd h:mm ap")
        }

        // What the core is doing, and how many questions are waiting for you.
        Process {
            id: ask
            command: ["/usr/local/bin/linuxai-core", "ctl", "{\"op\":\"waiting\"}"]
            stdout: StdioCollector {
                onStreamFinished: {
                    try {
                        const n = JSON.parse(this.text).plans.length
                        status.text = n ? `core running · ${n} waiting for you` : "core running"
                    } catch (e) { status.text = "core not answering" }
                }
            }
        }

        Timer {
            interval: 5000; running: true; repeat: true; triggeredOnStart: true
            onTriggered: { clock.text = Qt.formatDateTime(new Date(), "ddd h:mm ap"); ask.running = true }
        }
    }
}
