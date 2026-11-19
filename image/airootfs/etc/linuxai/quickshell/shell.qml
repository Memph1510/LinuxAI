// LinuxAI's always-on layer (Quickshell), above every app:
//   - a thin status line (the core, how many questions are waiting, the clock);
//   - the AI's questions, as cards with the answers to tap (same as replying to the text);
//   - the voice indicator (listening, what it heard, speaking);
//   - the AI's pointer: an outline and a note over whatever it's talking about, in any app. Never blocks clicks.
// Everything is driven by the core's event stream (`linuxai-core follow`), so it changes as things happen.
import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import QtQuick
import QtQuick.Layouts

ShellRoot {
    id: root
    property var questions: []          // [{plan_id, wait: {question, opts, why, lean}}]
    property string coreState: "checking…"
    property string voiceState: ""      // "", "listening", "heard", "speaking"
    property string voiceText: ""
    property var mark: null             // {x, y, w, h, note}

    readonly property color bg: "#0b1016"
    readonly property color panel: "#111821"
    readonly property color line: "#233140"
    readonly property color fg: "#dfe7ee"
    readonly property color dim: "#8ea0b1"
    readonly property color ai: "#46c2c9"
    readonly property color you: "#e8b04a"

    // ---- the core's events -------------------------------------------------------------------------------
    Process {
        id: follow
        command: ["/usr/local/bin/linuxai-core", "follow"]
        running: true
        stdout: SplitParser {
            onRead: data => {
                let ev
                try { ev = JSON.parse(data) } catch (e) { return }
                const k = ev.kind || ""
                if (k.startsWith("plan.") && ["plan.waiting", "plan.resumed", "plan.finished", "plan.stopped", "plan.failed", "plan.needs_thought"].includes(k))
                    waiting.running = true
                else if (k === "voice.listening") { root.voiceState = "listening"; root.voiceText = ""; voiceHide.stop() }
                else if (k === "voice.heard") { root.voiceState = "heard"; root.voiceText = ev.data.text || ""; voiceHide.restart() }
                else if (k === "voice.speaking") {
                    root.voiceState = "speaking"; root.voiceText = ev.data.text || ""
                    voiceHide.interval = Math.min(30000, 1500 + root.voiceText.length * 70); voiceHide.restart()
                }
                else if (k === "voice.done") root.voiceState = ""
                else if (k === "screen.point") { root.mark = ev.data; markHide.interval = (ev.data.seconds || 8) * 1000; markHide.restart() }
            }
        }
        // If the core restarts, follow again.
        onRunningChanged: if (!running) refollow.start()
    }
    Timer { id: refollow; interval: 3000; onTriggered: follow.running = true }
    Timer { id: voiceHide; interval: 5000; onTriggered: root.voiceState = "" }
    Timer { id: markHide; interval: 8000; onTriggered: root.mark = null }

    Process {
        id: waiting
        command: ["/usr/local/bin/linuxai-core", "ctl", "{\"op\":\"waiting\"}"]
        stdout: StdioCollector {
            onStreamFinished: {
                try {
                    const plans = JSON.parse(this.text).plans || []
                    root.questions = plans.filter(p => p.wait && p.wait.question)
                    root.coreState = "core running"
                } catch (e) { root.coreState = "core not answering" }
            }
        }
    }
    Timer { interval: 30000; running: true; repeat: true; triggeredOnStart: true; onTriggered: waiting.running = true }

    Process { id: answer }
    function reply(planId, text) {
        answer.command = ["/usr/local/bin/linuxai-core", "ctl", JSON.stringify({ op: "answer", plan_id: planId, text: text })]
        answer.running = true
        root.questions = root.questions.filter(q => q.plan_id !== planId)
    }

    // ---- the status line ---------------------------------------------------------------------------------
    PanelWindow {
        anchors { top: true; left: true; right: true }
        implicitHeight: 28
        color: root.bg

        Text {
            anchors.left: parent.left; anchors.leftMargin: 14; anchors.verticalCenter: parent.verticalCenter
            text: "Linux<font color='#46c2c9'>AI</font>"; textFormat: Text.RichText
            color: root.fg; font.pixelSize: 14; font.bold: true
        }
        Text {
            anchors.centerIn: parent; color: root.questions.length ? root.you : root.dim; font.pixelSize: 12
            text: root.questions.length ? `${root.coreState} · ${root.questions.length} waiting for you` : root.coreState
        }
        Text {
            id: clock
            anchors.right: parent.right; anchors.rightMargin: 14; anchors.verticalCenter: parent.verticalCenter
            color: root.fg; font.pixelSize: 12
            text: Qt.formatDateTime(new Date(), "ddd h:mm ap")
            Timer { interval: 15000; running: true; repeat: true; onTriggered: clock.text = Qt.formatDateTime(new Date(), "ddd h:mm ap") }
        }
    }

    // ---- the AI's questions ------------------------------------------------------------------------------
    PanelWindow {
        visible: root.questions.length > 0
        anchors { top: true; right: true }
        margins { top: 38; right: 12 }
        exclusionMode: ExclusionMode.Ignore
        implicitWidth: 380
        implicitHeight: cards.implicitHeight
        color: "transparent"

        ColumnLayout {
            id: cards
            width: parent.width
            spacing: 8
            Repeater {
                model: root.questions.slice(0, 3)
                delegate: Rectangle {
                    id: qcard
                    required property var modelData
                    Layout.fillWidth: true
                    implicitHeight: card.implicitHeight + 24
                    radius: 10; color: root.panel; border.color: root.you; border.width: 1
                    ColumnLayout {
                        id: card
                        anchors { left: parent.left; right: parent.right; top: parent.top; margins: 12 }
                        spacing: 6
                        Text { text: "LinuxAI needs you"; color: root.you; font.pixelSize: 11; font.bold: true }
                        Text { Layout.fillWidth: true; text: modelData.wait.question; color: root.fg; font.pixelSize: 14; wrapMode: Text.Wrap }
                        Text { Layout.fillWidth: true; visible: !!modelData.wait.why; text: "Why I'm unsure: " + (modelData.wait.why || ""); color: root.dim; font.pixelSize: 12; wrapMode: Text.Wrap }
                        Text { Layout.fillWidth: true; visible: !!modelData.wait.lean; text: "I'd lean: " + (modelData.wait.lean || ""); color: root.dim; font.pixelSize: 12; wrapMode: Text.Wrap }
                        Flow {
                            Layout.fillWidth: true; spacing: 6
                            Repeater {
                                model: modelData.wait.opts || ["yes", "no"]
                                delegate: Rectangle {
                                    required property var modelData
                                    required property int index
                                    radius: 6; implicitHeight: 28; implicitWidth: lbl.implicitWidth + 20
                                    color: index === 0 ? root.you : "transparent"; border.color: root.you
                                    Text { id: lbl; anchors.centerIn: parent; text: modelData; color: index === 0 ? "#2a1c02" : root.fg; font.pixelSize: 12 }
                                    MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: root.reply(qcard.modelData.plan_id, modelData) }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // ---- the voice indicator -----------------------------------------------------------------------------
    PanelWindow {
        visible: root.voiceState !== ""
        anchors { bottom: true }
        margins { bottom: 24 }
        exclusionMode: ExclusionMode.Ignore
        implicitWidth: Math.min(640, pill.implicitWidth + 32)
        implicitHeight: 44
        color: "transparent"
        mask: Region {}

        Rectangle {
            anchors.fill: parent; radius: 22
            color: root.panel
            border.color: root.voiceState === "speaking" ? root.ai : root.you; border.width: 2
            RowLayout {
                id: pill
                anchors.centerIn: parent; spacing: 10
                Rectangle {
                    width: 10; height: 10; radius: 5
                    color: root.voiceState === "speaking" ? root.ai : root.you
                    SequentialAnimation on opacity {
                        running: root.voiceState === "listening" || root.voiceState === "speaking"; loops: Animation.Infinite
                        NumberAnimation { to: 0.25; duration: 500 } NumberAnimation { to: 1; duration: 500 }
                    }
                }
                Text {
                    Layout.maximumWidth: 560; elide: Text.ElideRight
                    color: root.fg; font.pixelSize: 13
                    text: root.voiceState === "listening" ? "Listening… press Enter when you're done"
                        : root.voiceState === "heard" ? "You: " + root.voiceText
                        : "LinuxAI: " + root.voiceText
                }
            }
        }
    }

    // ---- the AI's pointer, over any app ------------------------------------------------------------------
    PanelWindow {
        visible: root.mark !== null
        anchors { top: true; bottom: true; left: true; right: true }
        exclusionMode: ExclusionMode.Ignore
        WlrLayershell.layer: WlrLayer.Overlay
        color: "transparent"
        mask: Region {}   // clicks go straight through to the app underneath

        Rectangle {
            visible: root.mark !== null
            x: root.mark ? root.mark.x - 6 : 0; y: root.mark ? root.mark.y - 6 : 0
            width: root.mark ? root.mark.w + 12 : 0; height: root.mark ? root.mark.h + 12 : 0
            radius: 8; color: "transparent"; border.color: root.you; border.width: 3
            Behavior on x { NumberAnimation { duration: 250 } }
            Behavior on y { NumberAnimation { duration: 250 } }
        }
        Rectangle {
            visible: root.mark !== null && !!root.mark.note
            x: root.mark ? root.mark.x : 0; y: root.mark ? root.mark.y + root.mark.h + 12 : 0
            width: note.implicitWidth + 20; height: note.implicitHeight + 14; radius: 8; color: "#1c1c1e"
            Text { id: note; anchors.centerIn: parent; text: root.mark ? root.mark.note : ""; color: "white"; font.pixelSize: 13 }
        }
    }
}
