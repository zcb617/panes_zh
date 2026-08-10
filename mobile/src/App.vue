<script setup lang="ts">
import { onLaunch, onShow, onHide } from "@dcloudio/uni-app";
onLaunch(() => {
  console.info("Panes Mobile launched");
});
onShow(() => {
  console.log("App Show");
});
onHide(() => {
  console.log("App Hide");
});
</script>
<style>
@import "./styles.css";

.markdown {
  white-space: pre-wrap;
  word-break: break-word;
}
</style>
