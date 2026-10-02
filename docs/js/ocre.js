// Ocre chrome for the mdBook pages: the logo and a link back to the home page
// (docs/site/index.html) in the menu bar and at the top of the sidebar.
(function () {
    "use strict";

    function brand(className) {
        var link = document.createElement("a");
        link.className = className;
        link.href = "/";
        var mark = document.createElement("img");
        mark.src = "/mark.svg";
        mark.alt = "";
        link.append(mark, document.createTextNode("ocre"));
        return link;
    }

    var title = document.querySelector(".menu-title");
    if (title) {
        title.replaceChildren(brand(""));
    }

    var right = document.querySelector("#mdbook-menu-bar .right-buttons");
    if (right) {
        var links = document.createElement("nav");
        links.className = "ocre-home-links";
        [["Home", "/"], ["API", "/api/ocre/"], ["llms.txt", "/llms.txt"]].forEach(function (item) {
            var a = document.createElement("a");
            a.textContent = item[0];
            a.href = item[1];
            links.append(a);
        });
        right.prepend(links);
    }

    // The sidebar is filled by mdBook's toc script, possibly after this one.
    var scrollbox = document.querySelector("mdbook-sidebar-scrollbox");
    function addSidebarBrand() {
        if (scrollbox && !scrollbox.querySelector(".ocre-brand") && scrollbox.firstElementChild) {
            scrollbox.prepend(brand("ocre-brand"));
            return true;
        }
        return false;
    }
    if (scrollbox && !addSidebarBrand()) {
        new MutationObserver(function (_, observer) {
            if (addSidebarBrand()) {
                observer.disconnect();
            }
        }).observe(scrollbox, { childList: true });
    }
})();
